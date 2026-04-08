use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::Color,
    Manager, PhysicalPosition,
};

#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;

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
        .plugin(tauri_plugin_shell::init())
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

            let window = app.get_webview_window("main").unwrap();
            window.set_background_color(Some(Color(0, 0, 0, 0))).unwrap();

            window.on_window_event({
                let window = window.clone();
                move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = window.hide();
                    }
                }
            });

            TrayIconBuilder::new()
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
                    if let TrayIconEvent::Click { position, button_state: MouseButtonState::Up, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let win_size = window.outer_size().unwrap();
                                let x = position.x as i32 - win_size.width as i32 / 2;
                                let y = position.y as i32;
                                let _ = window.set_position(PhysicalPosition::new(x, y));
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
