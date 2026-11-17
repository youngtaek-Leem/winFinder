use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum FileOperation {
    Copy {
        items: Vec<PathBuf>,
        destination: PathBuf,
    },
    Move {
        items: Vec<PathBuf>,
        destination: PathBuf,
    },
    Delete {
        items: Vec<PathBuf>,
        permanent: bool,
    },
    Rename {
        path: PathBuf,
        new_name: String,
    },
    CreateFolder {
        parent: PathBuf,
        name: String,
    },
}

pub fn perform(operation: &FileOperation) -> Result<(), String> {
    platform::perform(operation)
}

#[cfg(not(windows))]
mod platform {
    use super::FileOperation;
    use std::path::Path;
    use std::path::PathBuf;

    pub fn perform(operation: &FileOperation) -> Result<(), String> {
        match operation {
            FileOperation::Copy { items, destination } => {
                for item in items {
                    let target = destination.join(
                        item.file_name()
                            .ok_or_else(|| format!("이름이 없는 경로: {}", item.display()))?,
                    );
                    copy_path(item, &target)?;
                }
                Ok(())
            }
            FileOperation::Move { items, destination } => {
                for item in items {
                    let target = destination.join(
                        item.file_name()
                            .ok_or_else(|| format!("이름이 없는 경로: {}", item.display()))?,
                    );
                    if std::fs::rename(item, &target).is_err() {
                        copy_path(item, &target)?;
                        remove_path(item)?;
                    }
                }
                Ok(())
            }
            FileOperation::Delete { items, .. } => {
                for item in items {
                    remove_path(item)?;
                }
                Ok(())
            }
            FileOperation::Rename { path, new_name } => {
                let parent = path
                    .parent()
                    .ok_or_else(|| format!("부모 경로가 없습니다: {}", path.display()))?;
                std::fs::rename(path, parent.join(new_name)).map_err(|err| err.to_string())
            }
            FileOperation::CreateFolder { parent, name } => {
                std::fs::create_dir(parent.join(name)).map_err(|err| err.to_string())
            }
        }
    }

    fn copy_path(source: &Path, target: &Path) -> Result<(), String> {
        if source.is_dir() {
            std::fs::create_dir_all(target).map_err(|err| err.to_string())?;
            for item in std::fs::read_dir(source).map_err(|err| err.to_string())? {
                let item = item.map_err(|err| err.to_string())?;
                copy_path(&item.path(), &target.join(item.file_name()))?;
            }
            Ok(())
        } else {
            std::fs::copy(source, target)
                .map(|_| ())
                .map_err(|err| err.to_string())
        }
    }

    fn remove_path(path: &PathBuf) -> Result<(), String> {
        if path.is_dir() {
            std::fs::remove_dir_all(path).map_err(|err| err.to_string())
        } else {
            std::fs::remove_file(path).map_err(|err| err.to_string())
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::FileOperation;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_WANTNUKEWARNING, FOFX_ADDUNDORECORD,
        FileOperation as FileOperationClsid, IFileOperation, IFileOperationProgressSink,
        IShellItem, SHCreateItemFromParsingName,
    };
    use windows::core::PCWSTR;

    pub fn perform(operation: &FileOperation) -> Result<(), String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr.is_err() {
                return Err(format!("COM 초기화 실패: {hr:?}"));
            }
            let result = perform_inner(operation);
            CoUninitialize();
            result
        }
    }

    unsafe fn perform_inner(operation: &FileOperation) -> Result<(), String> {
        let file_operation: IFileOperation =
            CoCreateInstance(&FileOperationClsid, None, CLSCTX_ALL)
                .map_err(|err| format!("IFileOperation 생성 실패: {err}"))?;
        if let Some(hwnd) = crate::platform::window_handle() {
            unsafe {
                file_operation
                    .SetOwnerWindow(hwnd)
                    .map_err(|err| format!("소유 창 설정 실패: {err}"))?;
            }
        }

        let flags = match operation {
            FileOperation::Delete {
                permanent: true, ..
            } => FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING,
            FileOperation::Delete {
                permanent: false, ..
            } => FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOFX_ADDUNDORECORD,
            _ => FOF_ALLOWUNDO | FOFX_ADDUNDORECORD,
        };
        unsafe {
            file_operation
                .SetOperationFlags(flags)
                .map_err(|err| format!("파일 작업 플래그 설정 실패: {err}"))?;
        }

        match operation {
            FileOperation::Copy { items, destination } => {
                let destination = shell_item(destination)?;
                for item in items {
                    let source = shell_item(item)?;
                    unsafe {
                        file_operation
                            .CopyItem(
                                &source,
                                &destination,
                                PCWSTR::null(),
                                None::<&IFileOperationProgressSink>,
                            )
                            .map_err(|err| format!("복사 예약 실패: {err}"))?;
                    }
                }
            }
            FileOperation::Move { items, destination } => {
                let destination = shell_item(destination)?;
                for item in items {
                    let source = shell_item(item)?;
                    unsafe {
                        file_operation
                            .MoveItem(
                                &source,
                                &destination,
                                PCWSTR::null(),
                                None::<&IFileOperationProgressSink>,
                            )
                            .map_err(|err| format!("이동 예약 실패: {err}"))?;
                    }
                }
            }
            FileOperation::Delete { items, .. } => {
                for item in items {
                    let source = shell_item(item)?;
                    unsafe {
                        file_operation
                            .DeleteItem(&source, None::<&IFileOperationProgressSink>)
                            .map_err(|err| format!("삭제 예약 실패: {err}"))?;
                    }
                }
            }
            FileOperation::Rename { path, new_name } => {
                let source = shell_item(path)?;
                let name = wide(new_name);
                unsafe {
                    file_operation
                        .RenameItem(
                            &source,
                            PCWSTR(name.as_ptr()),
                            None::<&IFileOperationProgressSink>,
                        )
                        .map_err(|err| format!("이름 변경 예약 실패: {err}"))?;
                }
            }
            FileOperation::CreateFolder { parent, name } => {
                let folder = shell_item(parent)?;
                let name = wide(name);
                unsafe {
                    file_operation
                        .NewItem(
                            &folder,
                            FILE_ATTRIBUTE_DIRECTORY.0,
                            PCWSTR(name.as_ptr()),
                            PCWSTR::null(),
                            None::<&IFileOperationProgressSink>,
                        )
                        .map_err(|err| format!("새 폴더 예약 실패: {err}"))?;
                }
            }
        }

        unsafe {
            file_operation
                .PerformOperations()
                .map_err(|err| format!("파일 작업 실패: {err}"))?;
            if file_operation
                .GetAnyOperationsAborted()
                .map_err(|err| format!("파일 작업 상태 확인 실패: {err}"))?
                .as_bool()
            {
                return Err("파일 작업이 취소되었습니다".to_string());
            }
        }
        Ok(())
    }

    fn shell_item(path: &std::path::Path) -> Result<IShellItem, String> {
        unsafe {
            SHCreateItemFromParsingName(
                PCWSTR(wide_path(path).as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
            )
            .map_err(|err| format!("셸 항목 생성 실패 {}: {err}", path.display()))
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
