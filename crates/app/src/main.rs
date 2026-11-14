#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod video;

use eframe::egui;
use std::path::PathBuf;

fn main() {
    install_panic_hook();
    if let Err(error) = run() {
        report(
            "startup-error.log",
            &format!(
                "winFinder를 시작할 수 없습니다.\n\n{error}\n\n\
                 압축을 완전히 푼 뒤 다시 실행해 주세요.\
                 문제가 계속되면 이 창의 내용을 알려 주세요."
            ),
        );
    }
}

fn run() -> eframe::Result {
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

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        report(
            "crash.log",
            &format!(
                "winFinder가 예기치 않게 종료되었습니다.\n\n{info}\n\n\
                 이 창의 내용을 알려 주세요."
            ),
        );
    }));
}

fn report(file: &str, message: &str) {
    if let Some(path) = log_path(file) {
        let _ = std::fs::write(path, message);
    }
    show_message(message);
}

fn log_path(file: &str) -> Option<PathBuf> {
    let dir = winfinder_core::state::state_path()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(file))
}

#[cfg(windows)]
fn show_message(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;

    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = "winFinder"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_message(message: &str) {
    eprintln!("{message}");
}
