use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

// ─── C FFI for ScreenCaptureKit system audio (macOS only) ────────────────────

#[cfg(target_os = "macos")]
extern "C" {
    // Returns 0=ok, -1=permission denied, -2=other error
    fn capture_system_audio(
        path: *const std::ffi::c_char,
        max_secs: u64,
        stop_flag: *const u8, // points to AtomicBool storage (u8)
    ) -> std::ffi::c_int;
}

// ─── State ───────────────────────────────────────────────────────────────────

pub struct ScrobbleState {
    pub stop_flag:       Arc<AtomicBool>,
    pub selected_device: Mutex<Option<String>>,
    pub audio_source:    Mutex<String>, // "mic" | "system"
}

impl ScrobbleState {
    pub fn new() -> Self {
        Self {
            stop_flag:       Arc::new(AtomicBool::new(false)),
            selected_device: Mutex::new(None),
            audio_source:    Mutex::new("mic".to_string()),
        }
    }
}

// ─── Commands: device list / source ──────────────────────────────────────────

#[tauri::command]
pub fn get_audio_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

#[tauri::command]
pub fn set_audio_device(state: tauri::State<'_, ScrobbleState>, name: String) {
    *state.selected_device.lock().unwrap() = if name.is_empty() { None } else { Some(name) };
}

#[tauri::command]
pub fn get_audio_source(state: tauri::State<'_, ScrobbleState>) -> String {
    state.audio_source.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_audio_source(state: tauri::State<'_, ScrobbleState>, source: String) {
    *state.audio_source.lock().unwrap() = source;
}

// ─── Microphone capture (cpal) ───────────────────────────────────────────────

fn record_mic(
    path: &std::path::Path,
    stop_flag: Arc<AtomicBool>,
    max_secs: u64,
    device_name: Option<String>,
) -> Result<(), String> {
    let host = cpal::default_host();

    let device = match device_name {
        Some(ref name) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().ok().as_deref() == Some(name))
            .ok_or_else(|| format!("Audio device '{}' not found", name))?,
        None => host.default_input_device().ok_or("No input device available")?,
    };

    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels    = config.channels();
    let sample_rate = config.sample_rate().0;
    let sample_fmt  = config.sample_format();

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let writer = Arc::new(Mutex::new(
        Some(hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?)
    ));

    let stream = {
        let w = writer.clone();
        let s = stop_flag.clone();
        match sample_fmt {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    if s.load(Ordering::Relaxed) { return; }
                    if let Ok(mut g) = w.lock() {
                        if let Some(ref mut wtr) = *g {
                            for &v in data {
                                let _ = wtr.write_sample((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                            }
                        }
                    }
                },
                |_| {}, None,
            ).map_err(|e| e.to_string())?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
                    if s.load(Ordering::Relaxed) { return; }
                    if let Ok(mut g) = w.lock() {
                        if let Some(ref mut wtr) = *g {
                            for &v in data { let _ = wtr.write_sample(v); }
                        }
                    }
                },
                |_| {}, None,
            ).map_err(|e| e.to_string())?,
            _ => return Err("Unsupported audio sample format".to_string()),
        }
    };

    stream.play().map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < max_secs && !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
    if let Ok(mut g) = writer.lock() {
        if let Some(w) = g.take() { w.finalize().map_err(|e| e.to_string())?; }
    }
    Ok(())
}

// ─── System audio capture (ScreenCaptureKit, macOS 12.3+) ────────────────────

#[cfg(target_os = "macos")]
fn record_system_audio(
    path: &std::path::Path,
    stop_flag: Arc<AtomicBool>,
    max_secs: u64,
) -> Result<(), String> {
    use std::ffi::CString;
    let path_cstr = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| e.to_string())?;
    // AtomicBool stores a single u8; pass a pointer to that byte to C
    let flag_ptr = stop_flag.as_ptr() as *const u8;
    let rc = unsafe { capture_system_audio(path_cstr.as_ptr(), max_secs, flag_ptr) };
    match rc {
        0  => Ok(()),
        -1 => Err("screen_recording_permission".to_string()),
        _  => Err("system_audio_error".to_string()),
    }
}

// ─── Windows: WASAPI loopback ─────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn record_system_audio(
    path: &std::path::Path,
    stop_flag: Arc<AtomicBool>,
    max_secs: u64,
) -> Result<(), String> {
    use windows::{
        Win32::{
            Media::Audio::{
                eConsole, eRender, IAudioCaptureClient, IAudioClient,
                IMMDeviceEnumerator, MMDeviceEnumerator,
                AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
                WAVEFORMATEXTENSIBLE,
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree,
                CLSCTX_ALL, COINIT_MULTITHREADED,
            },
        },
        core::GUID,
    };

    // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    const SUBTYPE_FLOAT: GUID = GUID {
        data1: 0x00000003,
        data2: 0x0000,
        data3: 0x0010,
        data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
    };
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 2;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| e.to_string())?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;

        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;

        let pwfx = audio_client.GetMixFormat().map_err(|e| e.to_string())?;
        let fmt = &*pwfx;
        let channels    = fmt.nChannels;
        let sample_rate = fmt.nSamplesPerSec;
        let bits        = fmt.wBitsPerSample;

        let is_float = fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
            || (fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE && {
                let ext = &*(pwfx as *const _ as *const WAVEFORMATEXTENSIBLE);
                ext.SubFormat == SUBTYPE_FLOAT
            });

        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                10_000_000, // 1s buffer in 100ns units
                0,
                pwfx,
                None,
            )
            .map_err(|e| e.to_string())?;

        let capture: IAudioCaptureClient =
            audio_client.GetService().map_err(|e| e.to_string())?;

        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;

        audio_client.Start().map_err(|e| e.to_string())?;

        let start = std::time::Instant::now();
        loop {
            if stop_flag.load(Ordering::Relaxed) { break; }
            if start.elapsed().as_secs() >= max_secs { break; }

            std::thread::sleep(std::time::Duration::from_millis(10));

            let mut packet_len = capture.GetNextPacketSize().map_err(|e| e.to_string())?;
            while packet_len > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| e.to_string())?;

                if flags & AUDCLNT_BUFFERFLAGS_SILENT == 0 && frames > 0 {
                    let n = (frames * channels as u32) as usize;
                    if is_float {
                        let samples = std::slice::from_raw_parts(data as *const f32, n);
                        for &s in samples {
                            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            writer.write_sample(v).map_err(|e| e.to_string())?;
                        }
                    } else if bits == 16 {
                        let samples = std::slice::from_raw_parts(data as *const i16, n);
                        for &s in samples {
                            writer.write_sample(s).map_err(|e| e.to_string())?;
                        }
                    }
                }

                capture.ReleaseBuffer(frames).map_err(|e| e.to_string())?;
                packet_len = capture.GetNextPacketSize().map_err(|e| e.to_string())?;
            }
        }

        audio_client.Stop().ok();
        CoTaskMemFree(Some(pwfx as *mut _));
        writer.finalize().map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ─── Linux: PulseAudio/PipeWire monitor source ────────────────────────────────

#[cfg(target_os = "linux")]
fn record_system_audio(
    path: &std::path::Path,
    stop_flag: Arc<AtomicBool>,
    max_secs: u64,
) -> Result<(), String> {
    let host = cpal::default_host();
    // PulseAudio names monitor sources "Monitor of <device>"
    // PipeWire uses the same convention or "<device>.monitor"
    let monitor_name = host
        .input_devices()
        .map_err(|e| e.to_string())?
        .find(|d| {
            d.name().ok().map(|n| {
                let lower = n.to_lowercase();
                lower.starts_with("monitor of") || lower.ends_with(".monitor")
            }).unwrap_or(false)
        })
        .and_then(|d| d.name().ok())
        .ok_or_else(|| "no_monitor_device".to_string())?;

    record_mic(path, stop_flag, max_secs, Some(monitor_name))
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_scrobble(
    app: tauri::AppHandle,
    scrobble_state: tauri::State<'_, ScrobbleState>,
    lastfm_state: tauri::State<'_, crate::lastfm::LastfmState>,
) -> Result<serde_json::Value, String> {
    scrobble_state.stop_flag.store(false, Ordering::SeqCst);

    let tmp_path    = std::env::temp_dir().join("scrobloop_sample.wav");
    let stop_flag   = scrobble_state.stop_flag.clone();
    let path_clone  = tmp_path.clone();
    let source      = scrobble_state.audio_source.lock().unwrap().clone();
    let device_name = scrobble_state.selected_device.lock().unwrap().clone();

    if source == "system" {
        tauri::async_runtime::spawn_blocking(move || {
            record_system_audio(&path_clone, stop_flag, 12)
        })
        .await
        .map_err(|e| e.to_string())??;
    } else {
        tauri::async_runtime::spawn_blocking(move || {
            record_mic(&path_clone, stop_flag, 12, device_name)
        })
        .await
        .map_err(|e| e.to_string())??;
    }

    use tauri_plugin_shell::ShellExt;
    let path_str = tmp_path.to_string_lossy().into_owned();
    let output = app.shell()
        .sidecar("recognize-audio")
        .map_err(|e| e.to_string())?
        .args([path_str])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    std::fs::remove_file(&tmp_path).ok();

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout == "No match found." || stdout.is_empty() {
        return Err("no_match".to_string());
    }

    let track: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("parse error: {}", e))?;

    let sk = lastfm_state.session_key.lock().unwrap().clone();
    if let Some(session_key) = sk {
        let artist = track["artist"].as_str().unwrap_or("").to_string();
        let title  = track["title"].as_str().unwrap_or("").to_string();
        let album  = track["album"].as_str().unwrap_or("").to_string();
        crate::lastfm::scrobble_track(&session_key, &artist, &title, &album).await.ok();
    }

    Ok(track)
}

#[tauri::command]
pub fn stop_scrobble(state: tauri::State<'_, ScrobbleState>) {
    state.stop_flag.store(true, Ordering::SeqCst);
}
