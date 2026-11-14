#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;

use eframe::egui;

fn main() -> eframe::Result {
    let session = winfinder_core::Session::load().unwrap_or_default();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([session.window.width, session.window.height])
        .with_min_inner_size([760.0, 500.0])
        .with_title("winFinder");

    if let Some(position) = session.window.position {
        viewport = viewport.with_position(position);
    }
    if session.window.maximized {
        viewport = viewport.with_maximized(true);
    }

    let options = eframe::NativeOptions {
        viewport,
        centered: session.window.position.is_none(),
        ..Default::default()
    };

    eframe::run_native(
        "winFinder",
        options,
        Box::new(move |cc| Ok(Box::new(app::WinFinderApp::new(cc, session)))),
    )
}
