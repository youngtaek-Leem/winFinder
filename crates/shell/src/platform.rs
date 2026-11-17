use crate::{ClipboardFiles, DropEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, POINTL, WPARAM};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, DVASPECT_CONTENT,
    FORMATETC, IBindCtx, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropSource, IDropSource_Impl,
    IDropTarget, IDropTarget_Impl, OleFlushClipboard, OleGetClipboard, OleInitialize,
    OleSetClipboard, OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS,
};
use windows::Win32::UI::Shell::{
    BHID_SFObject, CMF_EXTENDEDVERBS, CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, ILFindLastID, ILFree,
    IShellFolder, IShellItem, IShellItemArray, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO,
    RemoveWindowSubclass, SHBindToParent, SHCreateDataObject, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromDataObject, SHDoDragDrop, SHMultiFileProperties, SHOP_FILEPATH,
    SHObjectProperties, SHOpenWithDialog, SHParseDisplayName, SIGDN_FILESYSPATH, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SW_SHOWNORMAL, SetForegroundWindow,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, TrackPopupMenuEx, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};
use windows::core::{BOOL as CoreBOOL, Interface, PCSTR, PCWSTR, PWSTR, Ref, implement};

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static OLE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static DROP_TARGET_REGISTERED: AtomicBool = AtomicBool::new(false);
static DROP_ACCEPTED: AtomicBool = AtomicBool::new(false);
static DROP_ALLOWED: AtomicU32 = AtomicU32::new(0);
static CLIPBOARD_CUT: AtomicBool = AtomicBool::new(false);
static INTERNAL_CLIPBOARD: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
static DROP_EVENTS: OnceLock<Mutex<Vec<DropEvent>>> = OnceLock::new();

const SUBCLASS_ID: usize = 0x7766_6e64;
const ID_CMD_FIRST: u32 = 1;
const ID_CMD_LAST: u32 = 0x7fff;
const CFSTR_PREFERREDDROPEFFECT: windows::core::PCWSTR = windows::core::w!("Preferred DropEffect");

pub fn init() -> Result<(), String> {
    unsafe {
        OleInitialize(None).map_err(|err| format!("OLE 초기화 실패: {err}"))?;
        OLE_INITIALIZED.store(true, Ordering::Release);
    }
    Ok(())
}

pub fn shutdown() {
    if DROP_TARGET_REGISTERED.swap(false, Ordering::AcqRel)
        && let Some(hwnd) = window_handle()
    {
        unsafe {
            let _ = RevokeDragDrop(hwnd);
        }
    }
    DROP_ACCEPTED.store(false, Ordering::Release);
    DROP_ALLOWED.store(0, Ordering::Release);
    if OLE_INITIALIZED.swap(false, Ordering::AcqRel) {
        unsafe {
            OleUninitialize();
        }
    }
}

pub fn attach_window(hwnd: isize) {
    WINDOW.store(hwnd, Ordering::Release);
}

pub(crate) fn window_handle() -> Option<HWND> {
    let raw = WINDOW.load(Ordering::Acquire);
    (raw != 0).then_some(HWND(raw as *mut core::ffi::c_void))
}

fn hwnd() -> Result<HWND, String> {
    window_handle().ok_or_else(|| "창 핸들을 찾을 수 없습니다".to_string())
}

pub fn register_drop_target() -> Result<(), String> {
    if DROP_TARGET_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    let hwnd = hwnd()?;
    let target: IDropTarget = DropTarget.into();
    unsafe {
        RegisterDragDrop(hwnd, &target).map_err(|err| format!("드롭 대상 등록 실패: {err}"))?;
    }
    DROP_TARGET_REGISTERED.store(true, Ordering::Release);
    std::mem::forget(target);
    Ok(())
}

pub fn take_drop_events() -> Vec<DropEvent> {
    let queue = DROP_EVENTS.get_or_init(|| Mutex::new(Vec::new()));
    queue
        .lock()
        .map(|mut events| std::mem::take(&mut *events))
        .unwrap_or_default()
}

fn push_drop_event(event: DropEvent) {
    let queue = DROP_EVENTS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut events) = queue.lock() {
        events.push(event);
    }
}

pub fn show_context_menu(
    paths: &[PathBuf],
    background: Option<&Path>,
    screen: (i32, i32),
) -> Result<(), String> {
    let hwnd = hwnd()?;
    unsafe {
        let menu = build_context_menu(paths, background, hwnd)?;
        let hmenu = CreatePopupMenu().map_err(|err| format!("메뉴 생성 실패: {err}"))?;
        let hr = menu.QueryContextMenu(
            hmenu,
            0,
            ID_CMD_FIRST,
            ID_CMD_LAST,
            CMF_NORMAL | CMF_EXTENDEDVERBS,
        );
        if hr.is_err() {
            let _ = DestroyMenu(hmenu);
            return Err(format!("셸 메뉴 구성 실패: {hr:?}"));
        }

        let context = Box::new(MenuContext {
            menu,
            menu2: None,
            menu3: None,
        });
        let mut context = context;
        context.menu2 = context.menu.cast::<IContextMenu2>().ok();
        context.menu3 = context.menu.cast::<IContextMenu3>().ok();
        let context_ptr = Box::into_raw(context);
        let _ = SetWindowSubclass(
            hwnd,
            Some(menu_subclass_proc),
            SUBCLASS_ID,
            context_ptr as usize,
        );
        let _ = SetForegroundWindow(hwnd);
        let (x, y) = if screen == (0, 0) {
            let mut point = windows::Win32::Foundation::POINT::default();
            let _ = GetCursorPos(&mut point);
            (point.x, point.y)
        } else {
            screen
        };
        let command = TrackPopupMenuEx(
            hmenu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            x,
            y,
            hwnd,
            None,
        )
        .0 as u32;
        let _ = RemoveWindowSubclass(hwnd, Some(menu_subclass_proc), SUBCLASS_ID);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);

        let result = if command != 0 {
            invoke_context_command(&(*context_ptr).menu, hwnd, command)
        } else {
            Ok(())
        };
        drop(Box::from_raw(context_ptr));
        result?;
    }
    let _ = hwnd;
    Ok(())
}

unsafe fn build_context_menu(
    paths: &[PathBuf],
    background: Option<&Path>,
    hwnd: HWND,
) -> Result<IContextMenu, String> {
    unsafe {
        if let Some(folder) = background {
            let item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(wide_path(folder).as_ptr()), None::<&IBindCtx>)
                    .map_err(|err| format!("배경 셸 항목 생성 실패: {err}"))?;
            let folder: IShellFolder = item
                .BindToHandler(None::<&IBindCtx>, &BHID_SFObject)
                .map_err(|err| format!("배경 폴더 바인딩 실패: {err}"))?;
            return folder
                .CreateViewObject(hwnd)
                .map_err(|err| format!("배경 메뉴 생성 실패: {err}"));
        }

        let first = paths
            .first()
            .ok_or_else(|| "메뉴를 표시할 항목이 없습니다".to_string())?;
        let mut absolute_pidls = Vec::with_capacity(paths.len());
        for path in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(
                PCWSTR(wide_path(path).as_ptr()),
                None::<&IBindCtx>,
                &mut pidl,
                0,
                None,
            )
            .map_err(|err| format!("PIDL 생성 실패 {}: {err}", path.display()))?;
            absolute_pidls.push(pidl);
        }

        let mut child = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(absolute_pidls[0] as *const _, Some(&mut child))
            .map_err(|err| format!("부모 폴더 바인딩 실패: {err}"))?;
        let children: Vec<*const _> = absolute_pidls
            .iter()
            .map(|pidl| ILFindLastID(*pidl) as *const _)
            .collect();
        let result = parent
            .GetUIObjectOf(hwnd, &children, None)
            .map_err(|err| format!("셸 메뉴 객체 생성 실패: {err}"));

        for pidl in absolute_pidls {
            ILFree(Some(pidl as *const _));
        }
        let _ = first;
        result
    }
}

unsafe fn invoke_context_command(
    menu: &IContextMenu,
    hwnd: HWND,
    command: u32,
) -> Result<(), String> {
    let offset = command.saturating_sub(ID_CMD_FIRST);
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: 0,
        hwnd,
        lpVerb: PCSTR(offset as usize as *const u8),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        menu.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
            .map_err(|err| format!("셸 메뉴 명령 실행 실패: {err}"))
    }
}

struct MenuContext {
    menu: IContextMenu,
    menu2: Option<IContextMenu2>,
    menu3: Option<IContextMenu3>,
}

unsafe extern "system" fn menu_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    refdata: usize,
) -> LRESULT {
    if refdata != 0
        && matches!(
            msg,
            WM_DRAWITEM | WM_MEASUREITEM | WM_INITMENUPOPUP | WM_MENUCHAR
        )
    {
        let context = &*(refdata as *const MenuContext);
        let mut result = LRESULT(0);
        if let Some(menu3) = &context.menu3 {
            let _ = unsafe { menu3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) };
            return result;
        }
        if let Some(menu2) = &context.menu2 {
            let _ = unsafe { menu2.HandleMenuMsg(msg, wparam, lparam) };
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

pub fn set_clipboard_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("복사할 항목이 없습니다".to_string());
    }
    let data = create_data_object(paths)?;
    let _ = set_preferred_drop_effect(
        &data,
        if cut {
            DROPEFFECT_MOVE.0
        } else {
            DROPEFFECT_COPY.0
        },
    );
    unsafe {
        OleSetClipboard(&data).map_err(|err| format!("클립보드 설정 실패: {err}"))?;
        let _ = OleFlushClipboard();
    }
    CLIPBOARD_CUT.store(cut, Ordering::Release);
    if let Ok(mut internal) = INTERNAL_CLIPBOARD
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        *internal = paths.to_vec();
    }
    Ok(())
}

pub fn clear_clipboard() -> Result<(), String> {
    unsafe {
        OleSetClipboard(None::<&IDataObject>)
            .map_err(|err| format!("클립보드 지우기 실패: {err}"))?;
    }
    CLIPBOARD_CUT.store(false, Ordering::Release);
    if let Ok(mut internal) = INTERNAL_CLIPBOARD
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        internal.clear();
    }
    Ok(())
}

pub fn clipboard_files() -> Result<Option<ClipboardFiles>, String> {
    let data = unsafe { OleGetClipboard() }.map_err(|err| format!("클립보드 읽기 실패: {err}"))?;
    let paths = unsafe { paths_from_data_object(&data) }.unwrap_or_default();
    if paths.is_empty() {
        return Ok(None);
    }
    let internal_cut = INTERNAL_CLIPBOARD
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|internal| {
            CLIPBOARD_CUT.load(Ordering::Acquire)
                && internal.len() == paths.len()
                && internal.iter().all(|path| paths.contains(path))
        })
        .unwrap_or(false);
    let external_cut = unsafe { preferred_drop_effect(&data) }
        .is_some_and(|effect| effect & DROPEFFECT_MOVE.0 != 0);
    Ok(Some(ClipboardFiles {
        paths,
        cut: internal_cut || external_cut,
    }))
}

pub fn show_properties(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("속성을 표시할 항목이 없습니다".to_string());
    }
    let hwnd = hwnd()?;
    if paths.len() == 1 {
        let path = wide_path(&paths[0]);
        let ok = unsafe {
            SHObjectProperties(
                Some(hwnd),
                SHOP_FILEPATH,
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
            )
        };
        if ok.as_bool() {
            Ok(())
        } else {
            Err("속성 창을 열 수 없습니다".to_string())
        }
    } else {
        let data = create_data_object(paths)?;
        unsafe { SHMultiFileProperties(&data, 0) }
            .map_err(|err| format!("속성 창을 열 수 없습니다: {err}"))
    }
}

pub fn open_with(path: &Path) -> Result<(), String> {
    let hwnd = hwnd()?;
    let file = wide_path(path);
    let info = OPENASINFO {
        pcszFile: PCWSTR(file.as_ptr()),
        pcszClass: PCWSTR::null(),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC,
    };
    unsafe { SHOpenWithDialog(Some(hwnd), &info) }
        .map_err(|err| format!("연결 프로그램 창을 열 수 없습니다: {err}"))
}

pub fn thumbnail(path: &Path, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return None;
        }
        let result = thumbnail_impl(path, size);
        CoUninitialize();
        result
    }
}

fn thumbnail_impl(path: &Path, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HDC, HGDIOBJ, ReleaseDC,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
    };

    unsafe {
        let item: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(wide_path(path).as_ptr()), None::<&IBindCtx>)
                .ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let hbitmap = factory
            .GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;
        let mut bitmap = BITMAP::default();
        let loaded = GetObjectW(
            HGDIOBJ(hbitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bitmap as *mut _ as *mut core::ffi::c_void),
        );
        if loaded == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
            return None;
        }
        let width = bitmap.bmWidth as u32;
        let height = bitmap.bmHeight as u32;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let dc: HDC = GetDC(None);
        if dc.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
            return None;
        }
        let lines = GetDIBits(
            dc,
            hbitmap,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(hbitmap.0));
        if lines == 0 {
            return None;
        }
        let mut has_alpha = false;
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            has_alpha |= pixel[3] != 0;
        }
        if !has_alpha {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        Some((pixels, width, height))
    }
}

pub fn start_drag(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let hwnd = hwnd()?;
    let data = create_data_object(paths)?;
    let source: IDropSource = DropSource.into();
    unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &data,
            &source,
            DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0),
        )
        .map_err(|err| format!("드래그 시작 실패: {err}"))?;
    }
    Ok(())
}

fn create_data_object(paths: &[PathBuf]) -> Result<IDataObject, String> {
    let first = paths
        .first()
        .ok_or_else(|| "데이터 개체를 만들 항목이 없습니다".to_string())?;
    let parent = first
        .parent()
        .ok_or_else(|| format!("부모 경로가 없습니다: {}", first.display()))?;
    unsafe {
        let mut parent_pidl = std::ptr::null_mut();
        SHParseDisplayName(
            PCWSTR(wide_path(parent).as_ptr()),
            None::<&IBindCtx>,
            &mut parent_pidl,
            0,
            None,
        )
        .map_err(|err| format!("부모 PIDL 생성 실패: {err}"))?;
        let mut absolute_pidls = Vec::with_capacity(paths.len());
        for path in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(
                PCWSTR(wide_path(path).as_ptr()),
                None::<&IBindCtx>,
                &mut pidl,
                0,
                None,
            )
            .map_err(|err| format!("PIDL 생성 실패 {}: {err}", path.display()))?;
            absolute_pidls.push(pidl);
        }
        let children: Vec<*const _> = absolute_pidls
            .iter()
            .map(|pidl| ILFindLastID(*pidl) as *const _)
            .collect();
        let data: IDataObject = SHCreateDataObject(
            Some(parent_pidl as *const _),
            Some(&children),
            None::<&IDataObject>,
        )
        .map_err(|err| format!("셸 데이터 개체 생성 실패: {err}"))?;
        for pidl in absolute_pidls {
            ILFree(Some(pidl as *const _));
        }
        ILFree(Some(parent_pidl as *const _));
        Ok(data)
    }
}

unsafe fn paths_from_data_object(data: &IDataObject) -> Result<Vec<PathBuf>, String> {
    unsafe {
        let array: IShellItemArray = SHCreateShellItemArrayFromDataObject(data)
            .map_err(|err| format!("셸 항목 배열 생성 실패: {err}"))?;
        let count = array
            .GetCount()
            .map_err(|err| format!("셸 항목 수 확인 실패: {err}"))?;
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let item = array
                .GetItemAt(index)
                .map_err(|err| format!("셸 항목 읽기 실패: {err}"))?;
            let name: PWSTR = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .map_err(|err| format!("파일 경로 확인 실패: {err}"))?;
            let path = name.to_string().map(PathBuf::from).map_err(|err| {
                CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
                format!("파일 경로 변환 실패: {err}")
            })?;
            CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
            paths.push(path);
        }
        Ok(paths)
    }
}

fn set_preferred_drop_effect(data: &IDataObject, effect: u32) -> Result<(), String> {
    unsafe {
        let format = windows::Win32::System::DataExchange::RegisterClipboardFormatW(
            CFSTR_PREFERREDDROPEFFECT,
        );
        if format == 0 {
            return Err("드롭 효과 형식을 등록할 수 없습니다".to_string());
        }
        let global = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>())
            .map_err(|err| format!("드롭 효과 메모리 할당 실패: {err}"))?;
        let pointer = GlobalLock(global);
        if pointer.is_null() {
            return Err("드롭 효과 메모리를 잠글 수 없습니다".to_string());
        }
        std::ptr::write_unaligned(pointer as *mut u32, effect);
        let _ = GlobalUnlock(global);
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: global },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        let format_etc = FORMATETC {
            cfFormat: format as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        data.SetData(&format_etc, &medium, true)
            .map_err(|err| format!("드롭 효과 설정 실패: {err}"))
    }
}

unsafe fn preferred_drop_effect(data: &IDataObject) -> Option<u32> {
    unsafe {
        let format = windows::Win32::System::DataExchange::RegisterClipboardFormatW(
            CFSTR_PREFERREDDROPEFFECT,
        );
        if format == 0 {
            return None;
        }
        let format_etc = FORMATETC {
            cfFormat: format as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = data.GetData(&format_etc).ok()?;
        let pointer = GlobalLock(HGLOBAL(medium.u.hGlobal.0));
        let effect = if pointer.is_null() {
            None
        } else {
            Some(std::ptr::read_unaligned(pointer as *const u32))
        };
        let _ = GlobalUnlock(HGLOBAL(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        effect
    }
}

#[implement(IDropTarget)]
struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accepted = data
            .as_ref()
            .and_then(|data| unsafe { paths_from_data_object(data) }.ok())
            .is_some_and(|paths| !paths.is_empty());
        DROP_ACCEPTED.store(accepted, Ordering::Release);
        let allowed = unsafe { *effect };
        DROP_ALLOWED.store(allowed.0, Ordering::Release);
        if accepted {
            push_drop_event(DropEvent::Enter {
                x: point.x,
                y: point.y,
            });
        }
        unsafe {
            *effect = if accepted {
                drop_effect(allowed, keys)
            } else {
                DROPEFFECT_NONE
            };
        }
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accepted = DROP_ACCEPTED.load(Ordering::Acquire);
        if accepted {
            push_drop_event(DropEvent::Over {
                x: point.x,
                y: point.y,
            });
        }
        unsafe {
            *effect = if accepted {
                drop_effect(DROPEFFECT(DROP_ALLOWED.load(Ordering::Acquire)), keys)
            } else {
                DROPEFFECT_NONE
            };
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        DROP_ACCEPTED.store(false, Ordering::Release);
        DROP_ALLOWED.store(0, Ordering::Release);
        push_drop_event(DropEvent::Leave);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accepted = DROP_ACCEPTED.swap(false, Ordering::AcqRel);
        let paths = data
            .as_ref()
            .and_then(|data| unsafe { paths_from_data_object(data) }.ok())
            .unwrap_or_default();
        let chosen = if accepted && !paths.is_empty() {
            drop_effect(DROPEFFECT(DROP_ALLOWED.load(Ordering::Acquire)), keys)
        } else {
            DROPEFFECT_NONE
        };
        DROP_ALLOWED.store(0, Ordering::Release);
        if chosen != DROPEFFECT_NONE {
            push_drop_event(DropEvent::Drop {
                paths,
                x: point.x,
                y: point.y,
                copy: chosen == DROPEFFECT_COPY,
            });
        }
        unsafe {
            *effect = chosen;
        }
        Ok(())
    }
}

fn drop_effect(allowed: DROPEFFECT, keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
    let copy = allowed.0 & DROPEFFECT_COPY.0 != 0;
    let move_ = allowed.0 & DROPEFFECT_MOVE.0 != 0;
    if keys.0 & MK_CONTROL.0 != 0 && copy {
        DROPEFFECT_COPY
    } else if move_ {
        DROPEFFECT_MOVE
    } else if copy {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(
        &self,
        escape: CoreBOOL,
        keys: MODIFIERKEYS_FLAGS,
    ) -> windows::core::HRESULT {
        if escape.as_bool() {
            return windows::Win32::Foundation::DRAGDROP_S_CANCEL;
        }
        let buttons = MK_LBUTTON.0 | MK_RBUTTON.0 | MK_MBUTTON.0;
        if keys.0 & buttons == 0 {
            windows::Win32::Foundation::DRAGDROP_S_DROP
        } else {
            windows::Win32::Foundation::S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> windows::core::HRESULT {
        windows::Win32::Foundation::DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
