mod assets;
#[allow(dead_code)] // Retain the tested native domain APIs during UI migration.
mod audio;
mod controller;
mod gui;
mod live_visual;
#[cfg(target_os = "macos")]
mod live_visual_metal;
#[allow(dead_code)] // Retain the tested native domain APIs during UI migration.
mod signal_summary;
#[allow(dead_code)] // Retain the tested native domain APIs during UI migration.
mod source;
#[allow(dead_code)] // Retain the tested native domain APIs during UI migration.
mod storage;
mod tags;
mod track_manager;
#[allow(dead_code)] // Retain the tested native domain APIs during UI migration.
mod transport;
use gpui::{prelude::*, *};
use gpui_component::Root;
fn main() -> anyhow::Result<()> {
    let _instance_lock = storage::acquire_instance_lock().map_err(anyhow::Error::msg)?;
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            gui::apply_theme(cx);
            gui::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1040.), px(680.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Cadence — local track review".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| gui::Cadence::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("open Cadence window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}
