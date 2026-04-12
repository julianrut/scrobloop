use std::collections::BTreeMap;
use std::sync::Mutex;
use tauri::Manager;

pub const API_KEY: &str = env!("LASTFM_API_KEY");
const API_SECRET: &str = env!("LASTFM_API_SECRET");
const API_BASE: &str = "https://ws.audioscrobbler.com/2.0/";

pub struct LastfmState {
    pub pending_token: Mutex<Option<String>>,
    pub session_key: Mutex<Option<String>>,
}

impl LastfmState {
    pub fn new() -> Self {
        Self {
            pending_token: Mutex::new(None),
            session_key: Mutex::new(None),
        }
    }
}

/// Compute Last.fm API signature: sorted key+value pairs + secret, MD5'd
fn api_sig(params: &BTreeMap<&str, String>) -> String {
    let mut s = String::new();
    for (k, v) in params {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(API_SECRET);
    format!("{:x}", md5::compute(s))
}

/// Step 1 — get a request token
pub async fn get_token() -> Result<String, String> {
    let mut params = BTreeMap::new();
    params.insert("api_key", API_KEY.to_string());
    params.insert("method", "auth.getToken".to_string());
    let sig = api_sig(&params);

    let resp = reqwest::Client::new()
        .get(API_BASE)
        .query(&[
            ("method", "auth.getToken"),
            ("api_key", API_KEY),
            ("api_sig", &sig),
            ("format", "json"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;

    resp["token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Last.fm error: {}", resp))
}

/// Step 2 — exchange token for session key (after user authorises in browser)
pub async fn get_session(token: &str) -> Result<(String, String), String> {
    let mut params = BTreeMap::new();
    params.insert("api_key", API_KEY.to_string());
    params.insert("method", "auth.getSession".to_string());
    params.insert("token", token.to_string());
    let sig = api_sig(&params);

    let resp = reqwest::Client::new()
        .get(API_BASE)
        .query(&[
            ("method", "auth.getSession"),
            ("api_key", API_KEY),
            ("token", token),
            ("api_sig", &sig),
            ("format", "json"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;

    let key = resp["session"]["key"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Not authorised yet: {}", resp))?;
    let name = resp["session"]["name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    Ok((key, name))
}

// ── Tauri commands ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_lastfm_auth(
    state: tauri::State<'_, LastfmState>,
) -> Result<(), String> {
    let token = get_token().await?;
    *state.pending_token.lock().unwrap() = Some(token.clone());
    let url = format!("https://www.last.fm/api/auth/?api_key={}&token={}", API_KEY, token);
    open::that(url).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_lastfm_auth_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, LastfmState>,
) -> Result<bool, String> {
    let token = state.pending_token.lock().unwrap().clone();
    let Some(token) = token else { return Ok(false) };

    match get_session(&token).await {
        Ok((session_key, _)) => {
            if let Ok(data_dir) = app.path().app_data_dir() {
                std::fs::create_dir_all::<&std::path::Path>(data_dir.as_ref()).ok();
                std::fs::write(data_dir.join(".session_key"), &session_key).ok();
            }
            *state.session_key.lock().unwrap() = Some(session_key);
            *state.pending_token.lock().unwrap() = None;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

#[tauri::command]
pub fn is_lastfm_authenticated(state: tauri::State<'_, LastfmState>) -> bool {
    state.session_key.lock().unwrap().is_some()
}

#[tauri::command]
pub fn logout(app: tauri::AppHandle, state: tauri::State<'_, LastfmState>) {
    *state.session_key.lock().unwrap() = None;
    *state.pending_token.lock().unwrap() = None;
    if let Ok(data_dir) = app.path().app_data_dir() {
        std::fs::remove_file(data_dir.join(".session_key")).ok();
    }
}

#[tauri::command]
pub async fn open_lastfm_profile(state: tauri::State<'_, LastfmState>) -> Result<(), String> {
    let sk = state.session_key.lock().unwrap().clone()
        .ok_or_else(|| "not authenticated".to_string())?;

    let resp = reqwest::Client::new()
        .get(API_BASE)
        .query(&[
            ("method", "user.getInfo"),
            ("api_key", API_KEY),
            ("sk", &sk),
            ("format", "json"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;

    let url = resp["user"]["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "could not get profile url".to_string())?;

    open::that(url).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_lastfm_username(state: tauri::State<'_, LastfmState>) -> Result<String, String> {
    let sk = state.session_key.lock().unwrap().clone()
        .ok_or_else(|| "not authenticated".to_string())?;

    let resp = reqwest::Client::new()
        .get(API_BASE)
        .query(&[
            ("method", "user.getInfo"),
            ("api_key", API_KEY),
            ("sk", &sk),
            ("format", "json"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;

    resp["user"]["name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("unexpected response: {}", resp))
}
