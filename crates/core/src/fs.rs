use crate::entry::Entry;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
pub fn list_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(path)? {
        let item = item?;
        let metadata = match item.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let name = item.file_name().to_string_lossy().into_owned();
        let item_path = item.path();
        let extension = item_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_lowercase();
        entries.push(Entry {
            name: name.clone(),
            path: item_path,
            is_dir: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            accessed: metadata.accessed().ok(),
            attributes: 0,
            hidden: name.starts_with('.'),
            extension,
        });
    }
    Ok(entries)
}

#[cfg(windows)]
pub fn list_dir(path: &Path) -> io::Result<Vec<Entry>> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM,
        FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch,
        FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
    };
    use windows::core::PCWSTR;

    let search = path.join("*");
    let wide: Vec<u16> = search
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut data = WIN32_FIND_DATAW::default();
    let handle = match unsafe {
        FindFirstFileExW(
            PCWSTR(wide.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    } {
        Ok(handle) => handle,
        Err(err) => {
            let code = err.code().0 & 0xFFFF;
            if code == 2 || code == 3 {
                return Ok(Vec::new());
            }
            return Err(io::Error::other(err.to_string()));
        }
    };

    let mut entries = Vec::new();
    loop {
        let name_len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name = String::from_utf16_lossy(&data.cFileName[..name_len]);
        if !name.is_empty() && name != "." && name != ".." {
            let attrs = data.dwFileAttributes;
            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
            let hidden = (attrs & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0)) != 0;
            let item_path = path.join(&name);
            let extension = item_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_lowercase();
            entries.push(Entry {
                name,
                path: item_path,
                is_dir,
                size: if is_dir {
                    0
                } else {
                    ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64
                },
                modified: filetime_to_systemtime(&data.ftLastWriteTime),
                created: filetime_to_systemtime(&data.ftCreationTime),
                accessed: filetime_to_systemtime(&data.ftLastAccessTime),
                attributes: attrs,
                hidden,
                extension,
            });
        }

        match unsafe { FindNextFileW(handle, &mut data) } {
            Ok(()) => continue,
            Err(err) => {
                let code = err.code().0 & 0xFFFF;
                if code != 18 {
                    unsafe {
                        let _ = FindClose(handle);
                    }
                    return Err(io::Error::other(err.to_string()));
                }
            }
        }
        break;
    }

    unsafe {
        let _ = FindClose(handle);
    }

    Ok(entries)
}

#[cfg(windows)]
fn filetime_to_systemtime(
    ft: &windows::Win32::Foundation::FILETIME,
) -> Option<std::time::SystemTime> {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    if ticks == 0 {
        return None;
    }
    const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
    let secs = ticks / 10_000_000;
    if secs < EPOCH_DIFF_SECS {
        return None;
    }
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs - EPOCH_DIFF_SECS))
}

#[cfg(windows)]
pub fn roots() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;
    let mask = unsafe { GetLogicalDrives() };
    let mut result = Vec::new();
    for i in 0..26 {
        if mask & (1 << i) != 0 {
            result.push(PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)));
        }
    }
    result
}

#[cfg(not(windows))]
pub fn roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

pub fn quick_access() -> Vec<(String, PathBuf)> {
    let mut items = Vec::new();
    if let Some(dirs) = directories::UserDirs::new() {
        items.push(("홈".to_string(), dirs.home_dir().to_path_buf()));
        if let Some(path) = dirs.desktop_dir() {
            items.push(("데스크톱".to_string(), path.to_path_buf()));
        }
        if let Some(path) = dirs.download_dir() {
            items.push(("다운로드".to_string(), path.to_path_buf()));
        }
        if let Some(path) = dirs.document_dir() {
            items.push(("문서".to_string(), path.to_path_buf()));
        }
    }
    items
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_current_directory() {
        let entries = list_dir(Path::new(".")).unwrap();
        assert!(!entries.is_empty());
    }
}
