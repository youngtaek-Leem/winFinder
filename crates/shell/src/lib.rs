#![allow(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;

pub mod file_operation;

pub use file_operation::FileOperation;

#[derive(Clone, Debug, Default)]
pub struct ClipboardFiles {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

#[derive(Clone, Debug)]
pub enum DropEvent {
    Enter {
        x: i32,
        y: i32,
    },
    Over {
        x: i32,
        y: i32,
    },
    Leave,
    Drop {
        paths: Vec<PathBuf>,
        x: i32,
        y: i32,
        copy: bool,
    },
}

pub fn init() -> Result<(), String> {
    platform::init()
}

pub fn shutdown() {
    platform::shutdown();
}

pub fn attach_window(hwnd: isize) {
    platform::attach_window(hwnd);
}

pub fn register_drop_target() -> Result<(), String> {
    platform::register_drop_target()
}

pub fn take_drop_events() -> Vec<DropEvent> {
    platform::take_drop_events()
}

pub fn show_context_menu(
    paths: &[PathBuf],
    background: Option<&std::path::Path>,
    screen: (i32, i32),
) -> Result<(), String> {
    platform::show_context_menu(paths, background, screen)
}

pub fn set_clipboard_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    platform::set_clipboard_files(paths, cut)
}

pub fn clipboard_files() -> Result<Option<ClipboardFiles>, String> {
    platform::clipboard_files()
}

pub fn clear_clipboard() -> Result<(), String> {
    platform::clear_clipboard()
}

pub fn show_properties(paths: &[PathBuf]) -> Result<(), String> {
    platform::show_properties(paths)
}

pub fn open_with(path: &std::path::Path) -> Result<(), String> {
    platform::open_with(path)
}

pub fn start_drag(paths: &[PathBuf]) -> Result<(), String> {
    platform::start_drag(paths)
}

pub fn thumbnail(path: &std::path::Path, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    platform::thumbnail(path, size)
}

#[cfg(not(windows))]
mod platform {
    use super::{ClipboardFiles, DropEvent};
    use std::path::{Path, PathBuf};

    pub fn init() -> Result<(), String> {
        Ok(())
    }

    pub fn shutdown() {}

    pub fn attach_window(_hwnd: isize) {}

    pub fn register_drop_target() -> Result<(), String> {
        Ok(())
    }

    pub fn take_drop_events() -> Vec<DropEvent> {
        Vec::new()
    }

    pub fn show_context_menu(
        _paths: &[PathBuf],
        _background: Option<&Path>,
        _screen: (i32, i32),
    ) -> Result<(), String> {
        Err("Windows 셸 메뉴는 Windows에서만 사용할 수 있습니다".to_string())
    }

    pub fn set_clipboard_files(_paths: &[PathBuf], _cut: bool) -> Result<(), String> {
        Err("파일 클립보드는 Windows에서만 사용할 수 있습니다".to_string())
    }

    pub fn clipboard_files() -> Result<Option<ClipboardFiles>, String> {
        Ok(None)
    }

    pub fn clear_clipboard() -> Result<(), String> {
        Ok(())
    }

    pub fn show_properties(_paths: &[PathBuf]) -> Result<(), String> {
        Err("속성 창은 Windows에서만 사용할 수 있습니다".to_string())
    }

    pub fn open_with(_path: &Path) -> Result<(), String> {
        Err("연결 프로그램은 Windows에서만 사용할 수 있습니다".to_string())
    }

    pub fn start_drag(_paths: &[PathBuf]) -> Result<(), String> {
        Err("드래그는 Windows에서만 사용할 수 있습니다".to_string())
    }

    pub fn thumbnail(_path: &Path, _size: u32) -> Option<(Vec<u8>, u32, u32)> {
        None
    }
}

#[cfg(windows)]
mod platform;
