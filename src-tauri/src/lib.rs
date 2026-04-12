mod lastfm;
use lastfm::LastfmState;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::Color,
    AppHandle, Manager, PhysicalPosition,
};

#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;

fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let Some(tray) = app.tray_by_id("main") else { return };
    let Ok(Some(rect)) = tray.rect() else { return };
    let win_size = window.outer_size().unwrap_or_default();
    let pos = rect.position.to_physical::<i32>(1.0);
    let size = rect.size.to_physical::<u32>(1.0);
    let x = pos.x + size.width as i32 / 2 - win_size.width as i32 / 2;
    let y = pos.y + size.height as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.show();
    let _ = window.set_focus();
}

fn is_online() -> bool {
    std::net::TcpStream::connect_timeout(
        &"1.1.1.1:53".parse().unwrap(),
        std::time::Duration::from_secs(2),
    )
    .is_ok()
}

fn tray_icon() -> Image<'static> {
    let svg = include_bytes!("../icons/icon_vector.svg");
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg, &opt).unwrap();
    let canvas = 128u32;
    let padding = 14u32;
    let icon_size = canvas - padding * 2;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(canvas, canvas).unwrap();
    let scale = icon_size as f32 / tree.size().width().max(tree.size().height());
    let offset_x = padding as f32 + (icon_size as f32 - tree.size().width() * scale) / 2.0;
    let offset_y = padding as f32 + (icon_size as f32 - tree.size().height() * scale) / 2.0;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y),
        &mut pixmap.as_mut(),
    );
    Image::new_owned(pixmap.data().to_vec(), canvas, canvas)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(LastfmState::new())
        .invoke_handler(tauri::generate_handler![
            lastfm::is_lastfm_authenticated,
            lastfm::start_lastfm_auth,
            lastfm::check_lastfm_auth_status,
            lastfm::get_lastfm_username,
        ])
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            let flag = app.path().app_data_dir().unwrap().join(".welcomed");
            if !flag.exists() {
                std::fs::create_dir_all(flag.parent().unwrap()).ok();
                std::fs::write(&flag, "").ok();
                use tauri_plugin_notification::NotificationExt;
                let notif = app.handle().notification();
                if notif.request_permission().is_ok() {
                    notif.builder()
                        .title("Welcome to Scrobloop 👋")
                        .body("This app lives in your system tray, click the Scrobloop icon")
                        .show()
                        .ok();
                }
            }

            let window = app.get_webview_window("main").unwrap();
            window.set_background_color(Some(Color(0, 0, 0, 0))).unwrap();

            let window_clone = window.clone();
            std::thread::spawn(move || {
                let mut last = is_online();
                if !last {
                    let _ = window_clone.eval("window.location.href = 'no_internet.html'");
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let online = is_online();
                    if online != last {
                        last = online;
                        let page = if online { "index.html" } else { "no_internet.html" };
                        let _ = window_clone.eval(&format!("window.location.href = '{page}'"));
                    }
                }
            });

            window.on_window_event({
                let window = window.clone();
                move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = window.hide();
                    }
                }
            });

            TrayIconBuilder::with_id("main")
                .icon(tray_icon())
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button_state: MouseButtonState::Up, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                show_window(app);
                            }
                        }
                    }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            use tauri_plugin_autostart::ManagerExt;
            let _ = app.autolaunch().enable();


            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
