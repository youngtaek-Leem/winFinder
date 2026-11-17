use crate::audio::AudioPlayer;
use crate::video::VideoPlayer;
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Key, Layout,
    Modifiers, Painter, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};
use winfinder_core::{
    ColumnWidths, Entry, History, LayoutPreset, Selection, SortKey, TabState, Theme, ViewMode,
    list_dir, quick_access, roots, sort_entries,
};
use winfinder_preview::{Preview, format_size};
use winfinder_shell::{ClipboardFiles, DropEvent, FileOperation};

const ROW_HEIGHT: f32 = 24.0;
const HEADER_HEIGHT: f32 = 22.0;
const PANE_GAP: f32 = 6.0;
const MIN_PANE_WIDTH: f32 = 220.0;
const MIN_PANE_HEIGHT: f32 = 140.0;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const STATUS_TTL: Duration = Duration::from_secs(4);
const AUTO_REFRESH: Duration = Duration::from_millis(2000);
const ICON_CELL: egui::Vec2 = egui::Vec2::new(92.0, 82.0);
const TILE_CELL: egui::Vec2 = egui::Vec2::new(170.0, 58.0);

pub struct WinFinderApp {
    panes: Vec<Pane>,
    preset: LayoutPreset,
    split_x: f32,
    split_y: f32,
    active_pane: usize,
    preview: PreviewState,
    cm_cache: egui_commonmark::CommonMarkCache,
    status: Option<(String, Instant)>,
    dirty: bool,
    last_save: Instant,
    window_state: winfinder_core::WindowState,
    theme: Theme,
    dialog: Option<Dialog>,
    next_tab_id: u64,
    load_tx: Sender<LoadEvent>,
    load_rx: Receiver<LoadEvent>,
    op_tx: Sender<OpResult>,
    op_rx: Receiver<OpResult>,
    clipboard: Option<ClipboardFiles>,
    pending_select: Option<PathBuf>,
    operations: usize,
    last_auto_refresh: Instant,
    drop_hover: Option<DropHover>,
    save_enabled: bool,
    thumbnails: ThumbnailStore,
}

struct Pane {
    tabs: Vec<Tab>,
    active: usize,
}

struct Tab {
    id: u64,
    generation: u64,
    loading: bool,
    history: History,
    entries: Vec<Entry>,
    view: Vec<usize>,
    selection: Selection,
    view_mode: ViewMode,
    columns: ColumnWidths,
    show_extensions: bool,
    sort: SortKey,
    sort_desc: bool,
    show_hidden: bool,
    scroll: f32,
    viewport_height: f32,
    row_pitch: f32,
    loaded: bool,
    error: Option<String>,
    address_editing: bool,
    address_buffer: String,
    address_focus: bool,
    content_rect: Rect,
    hit_rows: Vec<(Rect, PathBuf, bool)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    Startup,
    Navigate,
    Back,
    Forward,
    Refresh,
}

struct LoadEvent {
    tab_id: u64,
    generation: u64,
    path: PathBuf,
    kind: LoadKind,
    result: std::io::Result<Vec<Entry>>,
}

struct OpResult {
    success: bool,
    message: String,
    select: Option<PathBuf>,
    clear_clipboard: bool,
}

#[derive(Clone, Debug)]
struct DropHover {
    pane: usize,
    path: PathBuf,
}

struct ThumbnailEvent {
    path: PathBuf,
    result: Option<(Vec<u8>, u32, u32)>,
}

struct ThumbnailStore {
    textures: HashMap<PathBuf, TextureHandle>,
    pending: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    tx: Sender<ThumbnailEvent>,
    rx: Receiver<ThumbnailEvent>,
}

impl ThumbnailStore {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            textures: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            tx,
            rx,
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            self.pending.remove(&event.path);
            match event.result {
                Some((rgba, width, height)) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    let texture = ctx.load_texture(
                        format!("thumbnail:{}", event.path.to_string_lossy()),
                        image,
                        TextureOptions::LINEAR,
                    );
                    self.textures.insert(event.path, texture);
                }
                None => {
                    self.failed.insert(event.path);
                }
            }
            ctx.request_repaint();
        }
    }

    fn texture_for(&mut self, path: &Path) -> Option<TextureHandle> {
        if let Some(texture) = self.textures.get(path) {
            return Some(texture.clone());
        }
        if self.pending.contains(path) || self.failed.contains(path) || !thumbnail_candidate(path) {
            return None;
        }
        self.pending.insert(path.to_path_buf());
        let tx = self.tx.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let result = winfinder_shell::thumbnail(&path, 96)
                .or_else(|| winfinder_preview::load_thumbnail(&path, 96));
            let _ = tx.send(ThumbnailEvent { path, result });
        });
        None
    }
}

fn thumbnail_candidate(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "avif"
    ) {
        return true;
    }
    cfg!(windows)
        && matches!(
            extension.as_str(),
            "pdf"
                | "mp4"
                | "mkv"
                | "mov"
                | "avi"
                | "webm"
                | "wmv"
                | "m4v"
                | "mpg"
                | "mpeg"
                | "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "odt"
                | "ods"
                | "odp"
        )
}

#[derive(Default)]
struct PreviewState {
    open: bool,
    path: Option<PathBuf>,
    content: Option<Preview>,
    texture: Option<TextureHandle>,
    audio: Option<AudioPlayer>,
    video: Option<VideoPlayer>,
    video_error: Option<String>,
}

enum Dialog {
    Rename {
        path: PathBuf,
        name: String,
    },
    NewFolder {
        parent: PathBuf,
        name: String,
    },
    Delete {
        items: Vec<PathBuf>,
        permanent: bool,
    },
}

enum Action {
    Select(PathBuf),
    ToggleSelect(PathBuf),
    SelectRange(PathBuf, bool),
    Open(PathBuf),
    OpenInNewTab(PathBuf),
    OpenInOtherPane(PathBuf),
    OpenInNewWindow(PathBuf),
    Back,
    Forward,
    Up,
    NewTab,
    CloseTab(usize),
    SwitchTab(usize),
    Navigate(PathBuf),
    StartAddressEdit,
    SetView(ViewMode),
    SetHidden(bool),
    SetShowExtensions(bool),
    ColumnsChanged,
    SetSort(SortKey),
    SetSortDesc(bool),
    Refresh,
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(Vec<PathBuf>, bool),
    Preview(PathBuf),
    CopyPath(Vec<PathBuf>),
    Copy,
    Cut,
    Paste(PathBuf),
    Properties(Vec<PathBuf>),
    OpenWith(PathBuf),
    ShellMenu {
        paths: Vec<PathBuf>,
        background: Option<PathBuf>,
    },
    StartDrag(Vec<PathBuf>),
}

impl Tab {
    fn from_state(state: TabState, id: u64) -> Self {
        let mut selection = Selection::default();
        let stored = if state.selection.is_empty() {
            state.selected.into_iter().collect::<Vec<_>>()
        } else {
            state.selection
        };
        selection.replace(stored, None);
        Self {
            id,
            generation: 0,
            loading: false,
            history: state.history,
            entries: Vec::new(),
            view: Vec::new(),
            selection,
            view_mode: state.view,
            columns: state.columns.normalized(),
            show_extensions: state.show_extensions,
            sort: state.sort,
            sort_desc: state.sort_desc,
            show_hidden: state.show_hidden,
            scroll: state.scroll,
            viewport_height: 0.0,
            row_pitch: 0.0,
            loaded: false,
            error: None,
            address_editing: false,
            address_buffer: String::new(),
            address_focus: false,
            content_rect: Rect::NOTHING,
            hit_rows: Vec::new(),
        }
    }

    fn to_state(&self) -> TabState {
        TabState {
            history: self.history.clone(),
            sort: self.sort,
            sort_desc: self.sort_desc,
            view: self.view_mode,
            columns: self.columns,
            show_extensions: self.show_extensions,
            show_hidden: self.show_hidden,
            selection: self.selection.paths(),
            selected: self.selection.primary(),
            scroll: self.scroll,
        }
    }

    fn display_name(&self) -> String {
        self.history
            .current()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .or_else(|| {
                self.history
                    .current()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_else(|| "?".to_string())
    }

    fn apply_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.error = None;
        self.loaded = true;
        self.apply_sort();
    }

    fn apply_sort(&mut self) {
        sort_entries(&mut self.entries, self.sort, self.sort_desc);
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        self.view = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.show_hidden || !entry.is_hidden())
            .map(|(index, _)| index)
            .collect();
        let visible: BTreeSet<PathBuf> = self
            .view
            .iter()
            .map(|&index| self.entries[index].path.clone())
            .collect();
        self.selection.retain_existing(&visible);
    }

    fn ordered_paths(&self) -> Vec<PathBuf> {
        self.view
            .iter()
            .map(|&index| self.entries[index].path.clone())
            .collect()
    }

    fn focus_index(&self) -> Option<usize> {
        let focus = self.selection.focus()?;
        self.view
            .iter()
            .position(|&index| self.entries[index].path.as_path() == focus)
    }

    fn start_address_edit(&mut self) {
        self.address_editing = true;
        self.address_focus = true;
        self.address_buffer = self
            .history
            .current()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
}

impl Pane {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

impl WinFinderApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mut session: winfinder_core::Session,
        ephemeral: bool,
    ) -> Self {
        install_fonts(&cc.egui_ctx);
        session.normalize();
        apply_theme(&cc.egui_ctx, session.theme);
        let shell_error = attach_shell(cc);
        let (load_tx, load_rx) = channel();
        let (op_tx, op_rx) = channel();
        let mut next_tab_id = 1;
        let mut panes = Vec::new();
        for pane_state in session.panes {
            let mut tabs: Vec<Tab> = pane_state
                .tabs
                .into_iter()
                .map(|state| {
                    let id = next_tab_id;
                    next_tab_id += 1;
                    Tab::from_state(state, id)
                })
                .collect();
            if tabs.is_empty() {
                tabs.push(Tab::from_state(TabState::default(), next_tab_id));
                next_tab_id += 1;
            }
            let active = pane_state.active_tab.min(tabs.len() - 1);
            panes.push(Pane { tabs, active });
        }

        let mut app = Self {
            panes,
            preset: session.preset,
            split_x: session.split_x,
            split_y: session.split_y,
            active_pane: session.active_pane,
            preview: PreviewState::default(),
            cm_cache: egui_commonmark::CommonMarkCache::default(),
            status: shell_error.map(|error| (error, Instant::now())),
            dirty: false,
            last_save: Instant::now(),
            window_state: session.window,
            theme: session.theme,
            dialog: None,
            next_tab_id,
            load_tx,
            load_rx,
            op_tx,
            op_rx,
            clipboard: None,
            pending_select: None,
            operations: 0,
            last_auto_refresh: Instant::now(),
            drop_hover: None,
            save_enabled: !ephemeral && std::env::var_os("WINFINDER_NO_SAVE").is_none(),
            thumbnails: ThumbnailStore::new(),
        };
        for pane_index in 0..app.panes.len() {
            let Some(path) = app.panes[pane_index]
                .active_tab()
                .history
                .current()
                .cloned()
            else {
                continue;
            };
            app.request_load(pane_index, path, LoadKind::Startup);
        }
        app
    }

    fn tab_by_id_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.panes
            .iter_mut()
            .flat_map(|pane| pane.tabs.iter_mut())
            .find(|tab| tab.id == id)
    }

    fn request_load(&mut self, pane_idx: usize, path: PathBuf, kind: LoadKind) {
        let Some(pane) = self.panes.get_mut(pane_idx) else {
            return;
        };
        let tab = pane.active_tab_mut();
        tab.generation = tab.generation.wrapping_add(1);
        tab.loading = true;
        tab.error = None;
        let generation = tab.generation;
        let tab_id = tab.id;
        let event_path = path.clone();
        let tx = self.load_tx.clone();
        std::thread::spawn(move || {
            let result = list_dir(&event_path);
            let _ = tx.send(LoadEvent {
                tab_id,
                generation,
                path: event_path,
                kind,
                result,
            });
        });
    }

    fn apply_load_event(&mut self, event: LoadEvent, ctx: &egui::Context) {
        let pending = self.pending_select.take();
        let mut selected_pending = false;
        let mut success = false;
        let mut error_message = None;
        {
            let Some(tab) = self.tab_by_id_mut(event.tab_id) else {
                self.pending_select = pending;
                return;
            };
            if tab.generation != event.generation {
                self.pending_select = pending;
                return;
            }
            tab.loading = false;
            match event.result {
                Ok(entries) => {
                    success = true;
                    match event.kind {
                        LoadKind::Navigate => {
                            tab.history.push(event.path.clone());
                            tab.scroll = 0.0;
                            tab.selection.clear();
                        }
                        LoadKind::Back => {
                            tab.history.back();
                            tab.scroll = 0.0;
                            tab.selection.clear();
                        }
                        LoadKind::Forward => {
                            tab.history.forward();
                            tab.scroll = 0.0;
                            tab.selection.clear();
                        }
                        LoadKind::Startup | LoadKind::Refresh => {}
                    }
                    tab.apply_entries(entries);
                    tab.address_editing = false;
                    if let Some(target) = pending.as_ref()
                        && target.parent() == tab.history.current().map(PathBuf::as_path)
                        && tab.entries.iter().any(|entry| &entry.path == target)
                    {
                        tab.selection.select_only(target.clone());
                        selected_pending = true;
                    }
                }
                Err(err) => {
                    tab.error = Some(format!("{}: {err}", event.path.display()));
                    tab.loaded = true;
                    error_message = Some(format!("폴더를 열 수 없습니다: {err}"));
                }
            }
        }
        if !selected_pending {
            self.pending_select = pending;
        }
        if success {
            self.dirty = true;
            self.after_selection_changed(ctx);
        }
        if let Some(message) = error_message {
            self.set_status(message);
        }
    }

    fn poll_events(&mut self, ctx: &egui::Context) {
        self.thumbnails.poll(ctx);
        while let Ok(event) = self.load_rx.try_recv() {
            self.apply_load_event(event, ctx);
        }
        while let Ok(result) = self.op_rx.try_recv() {
            self.operations = self.operations.saturating_sub(1);
            self.set_status(result.message);
            if result.success {
                if result.clear_clipboard {
                    self.clipboard = None;
                    let _ = winfinder_shell::clear_clipboard();
                }
                if let Some(path) = result.select {
                    self.pending_select = Some(path);
                }
                for pane_index in 0..self.panes.len() {
                    if let Some(path) = self.panes[pane_index]
                        .active_tab()
                        .history
                        .current()
                        .cloned()
                    {
                        self.request_load(pane_index, path, LoadKind::Refresh);
                    }
                }
            }
        }
        for event in winfinder_shell::take_drop_events() {
            match event {
                DropEvent::Enter { x, y } | DropEvent::Over { x, y } => {
                    self.drop_hover = self.drop_target(ctx, x, y);
                    ctx.request_repaint();
                }
                DropEvent::Leave => {
                    self.drop_hover = None;
                    ctx.request_repaint();
                }
                DropEvent::Drop { paths, x, y, copy } => {
                    self.drop_hover = None;
                    if paths.is_empty() {
                        continue;
                    }
                    let target =
                        self.drop_target(ctx, x, y)
                            .map(|hover| hover.path)
                            .or_else(|| {
                                self.panes
                                    .get(self.active_pane)
                                    .and_then(|pane| pane.active_tab().history.current().cloned())
                            });
                    if let Some(target) = target {
                        let operation = if copy {
                            FileOperation::Copy {
                                items: paths,
                                destination: target,
                            }
                        } else {
                            FileOperation::Move {
                                items: paths,
                                destination: target,
                            }
                        };
                        self.start_file_operation(operation, "파일을 이동했습니다", None, false);
                    }
                }
            }
        }
    }

    fn screen_to_points(&self, ctx: &egui::Context, x: i32, y: i32) -> egui::Pos2 {
        let ppp = ctx.pixels_per_point();
        let origin = ctx
            .input(|input| {
                let viewport = input.viewport();
                viewport.inner_rect.or(viewport.outer_rect)
            })
            .map(|rect| rect.min)
            .unwrap_or_default();
        egui::pos2(x as f32 / ppp - origin.x, y as f32 / ppp - origin.y)
    }

    fn drop_target(&self, ctx: &egui::Context, x: i32, y: i32) -> Option<DropHover> {
        let point = self.screen_to_points(ctx, x, y);
        for (pane_index, pane) in self.panes.iter().enumerate() {
            for (tab_index, tab) in pane.tabs.iter().enumerate() {
                if pane.active != tab_index || !tab.content_rect.contains(point) {
                    continue;
                }
                let target = tab
                    .hit_rows
                    .iter()
                    .find(|(rect, _, is_dir)| *is_dir && rect.contains(point))
                    .map(|(_, path, _)| path.clone())
                    .or_else(|| tab.history.current().cloned());
                if let Some(path) = target {
                    return Some(DropHover {
                        pane: pane_index,
                        path,
                    });
                }
            }
        }
        None
    }

    fn start_file_operation(
        &mut self,
        operation: FileOperation,
        message: &str,
        select: Option<PathBuf>,
        clear_clipboard: bool,
    ) {
        self.operations += 1;
        self.set_status("파일 작업 중...");
        let tx = self.op_tx.clone();
        let success_message = message.to_string();
        std::thread::spawn(move || {
            let result = winfinder_shell::file_operation::perform(&operation);
            let (success, message) = match result {
                Ok(()) => (true, success_message),
                Err(err) => (false, format!("작업 실패: {err}")),
            };
            let _ = tx.send(OpResult {
                success,
                message,
                select,
                clear_clipboard,
            });
        });
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.panes
            .get(self.active_pane)?
            .active_tab()
            .selection
            .primary()
    }

    fn selected_paths(&self, pane_idx: usize) -> Vec<PathBuf> {
        self.panes
            .get(pane_idx)
            .map(|pane| pane.active_tab().selection.paths())
            .unwrap_or_default()
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.theme = match self.theme {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        };
        apply_theme(ctx, self.theme);
        self.dirty = true;
    }

    fn toggle_preview(&mut self, ctx: &egui::Context) {
        if self.preview.open {
            self.close_preview();
            return;
        }
        self.show_preview(ctx);
    }

    fn show_preview(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_path() else {
            self.set_status("미리보기할 파일을 선택하세요");
            return;
        };
        if path.is_dir() {
            self.set_status("폴더는 미리보기를 지원하지 않습니다");
            return;
        }
        self.preview.open = true;
        self.refresh_preview(ctx);
    }

    fn close_preview(&mut self) {
        self.preview.open = false;
        self.preview.content = None;
        self.preview.texture = None;
        self.preview.path = None;
        self.preview.audio = None;
        self.preview.video = None;
        self.preview.video_error = None;
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_path() else {
            self.close_preview();
            return;
        };
        if path.is_dir() {
            self.close_preview();
            return;
        }
        if self.preview.path.as_ref() == Some(&path) && self.preview.content.is_some() {
            return;
        }

        let preview = winfinder_preview::load(&path);
        let image = match &preview {
            Preview::Image {
                rgba,
                width,
                height,
            }
            | Preview::Pdf {
                rgba,
                width,
                height,
                ..
            } => Some((rgba, *width, *height)),
            _ => None,
        };
        let texture = image.map(|(rgba, width, height)| {
            let color =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba);
            ctx.load_texture(
                format!("preview:{}", path.to_string_lossy()),
                color,
                TextureOptions::LINEAR,
            )
        });

        self.preview.audio = None;
        let audio = if matches!(preview, Preview::Audio { .. }) {
            match AudioPlayer::open(&path) {
                Ok(player) => Some(player),
                Err(err) => {
                    self.set_status(err);
                    None
                }
            }
        } else {
            None
        };

        self.preview.video = None;
        self.preview.video_error = None;
        let video = if matches!(preview, Preview::Video { .. }) {
            match VideoPlayer::open(&path) {
                Ok(player) => Some(player),
                Err(error) => {
                    self.preview.video_error = Some(error);
                    None
                }
            }
        } else {
            None
        };

        self.preview.path = Some(path);
        self.preview.content = Some(preview);
        self.preview.texture = texture;
        self.preview.audio = audio;
        self.preview.video = video;
    }

    fn after_selection_changed(&mut self, ctx: &egui::Context) {
        if !self.preview.open {
            return;
        }
        match self.selected_path() {
            Some(path) if path.is_file() => self.refresh_preview(ctx),
            _ => self.close_preview(),
        }
    }

    fn navigate(&mut self, ctx: &egui::Context, pane_idx: usize, path: PathBuf) {
        if path.as_os_str().is_empty() {
            return;
        }
        let mut path = expand_home(path);
        if path.is_relative()
            && let Some(current) = self
                .panes
                .get(pane_idx)
                .and_then(|pane| pane.active_tab().history.current().cloned())
        {
            path = current.join(path);
        }
        self.request_load(pane_idx, path, LoadKind::Navigate);
        self.close_preview();
        self.dirty = true;
        ctx.request_repaint();
    }

    fn go_back(&mut self, ctx: &egui::Context, pane_idx: usize) {
        let target = self
            .panes
            .get(pane_idx)
            .and_then(|pane| pane.active_tab().history.back_target());
        if let Some(path) = target {
            self.request_load(pane_idx, path, LoadKind::Back);
            self.close_preview();
            self.dirty = true;
            ctx.request_repaint();
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context, pane_idx: usize) {
        let target = self
            .panes
            .get(pane_idx)
            .and_then(|pane| pane.active_tab().history.forward_target());
        if let Some(path) = target {
            self.request_load(pane_idx, path, LoadKind::Forward);
            self.close_preview();
            self.dirty = true;
            ctx.request_repaint();
        }
    }

    fn go_up(&mut self, ctx: &egui::Context, pane_idx: usize) {
        let target = self
            .panes
            .get(pane_idx)
            .and_then(|pane| pane.active_tab().history.parent());
        if let Some(path) = target {
            self.navigate(ctx, pane_idx, path);
        }
    }

    fn open_path(&mut self, ctx: &egui::Context, pane_idx: usize, path: PathBuf) {
        if path.is_dir() {
            self.navigate(ctx, pane_idx, path);
        } else if let Err(err) = open::that(&path) {
            self.set_status(format!("열기 실패: {err}"));
        }
    }

    fn new_tab(&mut self, pane_idx: usize) {
        let current = self
            .panes
            .get(pane_idx)
            .and_then(|pane| pane.active_tab().history.current().cloned())
            .unwrap_or_else(winfinder_core::state::default_dir);
        self.new_tab_at(pane_idx, current);
    }

    fn new_tab_at(&mut self, pane_idx: usize, path: PathBuf) {
        let Some(pane) = self.panes.get_mut(pane_idx) else {
            return;
        };
        if pane.tabs.len() >= winfinder_core::MAX_TABS_PER_PANE {
            self.set_status(format!(
                "탭은 최대 {}개입니다",
                winfinder_core::MAX_TABS_PER_PANE
            ));
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let state = TabState {
            history: History::new(path.clone()),
            ..TabState::default()
        };
        pane.tabs.push(Tab::from_state(state, id));
        pane.active = pane.tabs.len() - 1;
        self.request_load(pane_idx, path, LoadKind::Startup);
        self.dirty = true;
    }

    fn close_tab(&mut self, pane_idx: usize, index: usize) {
        let Some(pane) = self.panes.get_mut(pane_idx) else {
            return;
        };
        if pane.tabs.len() <= 1 {
            self.set_status("마지막 탭은 닫을 수 없습니다");
            return;
        }
        if index >= pane.tabs.len() {
            return;
        }
        pane.tabs.remove(index);
        if pane.active >= pane.tabs.len() {
            pane.active = pane.tabs.len() - 1;
        } else if index < pane.active {
            pane.active -= 1;
        }
        if let Some(path) = pane.active_tab().history.current().cloned() {
            self.request_load(pane_idx, path, LoadKind::Refresh);
        }
        self.dirty = true;
        self.close_preview();
    }

    fn switch_tab(&mut self, pane_idx: usize, index: usize) {
        let target = {
            let Some(pane) = self.panes.get_mut(pane_idx) else {
                return;
            };
            if index >= pane.tabs.len() {
                return;
            }
            pane.active = index;
            pane.active_tab().history.current().cloned()
        };
        if let Some(path) = target {
            self.request_load(pane_idx, path, LoadKind::Refresh);
        }
        self.dirty = true;
        self.close_preview();
    }

    fn reload_pane(&mut self, pane_idx: usize) {
        if let Some(path) = self
            .panes
            .get(pane_idx)
            .and_then(|pane| pane.active_tab().history.current().cloned())
        {
            self.request_load(pane_idx, path, LoadKind::Refresh);
        }
        self.dirty = true;
    }

    fn reload_active(&mut self) {
        self.reload_pane(self.active_pane);
    }

    fn new_window(&mut self, path: PathBuf) {
        let Ok(exe) = std::env::current_exe() else {
            self.set_status("실행 파일 경로를 찾을 수 없습니다");
            return;
        };
        match std::process::Command::new(exe)
            .arg("--path")
            .arg(path)
            .env("WINFINDER_NO_SAVE", "1")
            .spawn()
        {
            Ok(_) => self.set_status("새 창을 열었습니다"),
            Err(err) => self.set_status(format!("새 창 열기 실패: {err}")),
        }
    }

    fn move_selection(&mut self, ctx: &egui::Context, delta: isize, extend: bool) {
        let changed = {
            let Some(pane) = self.panes.get_mut(self.active_pane) else {
                return;
            };
            let tab = pane.active_tab_mut();
            if tab.view.is_empty() {
                return;
            }
            let current = tab.focus_index();
            let next = match current {
                Some(position) => {
                    (position as isize + delta).clamp(0, tab.view.len() as isize - 1) as usize
                }
                None => {
                    if delta > 0 {
                        0
                    } else {
                        tab.view.len() - 1
                    }
                }
            };
            let path = tab.entries[tab.view[next]].path.clone();
            if extend {
                let ordered = tab.ordered_paths();
                tab.selection.select_range(&ordered, next, false);
            } else {
                tab.selection.select_only(path);
            }
            tab.scroll = scroll_to_show_row(
                next,
                tab.scroll,
                tab.viewport_height,
                tab.row_pitch.max(ROW_HEIGHT),
            );
            true
        };
        if changed {
            self.dirty = true;
            self.after_selection_changed(ctx);
        }
    }

    fn open_selected(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_path() else {
            return;
        };
        self.open_path(ctx, self.active_pane, path);
    }

    fn copy_selection(&mut self, cut: bool) {
        let paths = self.selected_paths(self.active_pane);
        if paths.is_empty() {
            self.set_status("복사할 항목을 선택하세요");
            return;
        }
        match winfinder_shell::set_clipboard_files(&paths, cut) {
            Ok(()) => self.set_status(if cut {
                "잘라내기 했습니다"
            } else {
                "복사했습니다"
            }),
            Err(_) => self.set_status(if cut {
                "내부 클립보드에 잘라내기 했습니다"
            } else {
                "내부 클립보드에 복사했습니다"
            }),
        }
        self.clipboard = Some(ClipboardFiles { paths, cut });
        self.dirty = true;
    }

    fn paste_into(&mut self, destination: PathBuf) {
        let clipboard = winfinder_shell::clipboard_files()
            .ok()
            .flatten()
            .or_else(|| self.clipboard.clone());
        let Some(clipboard) = clipboard else {
            self.set_status("붙여넣을 항목이 없습니다");
            return;
        };
        if clipboard.paths.is_empty() {
            self.set_status("붙여넣을 항목이 없습니다");
            return;
        }
        if clipboard
            .paths
            .iter()
            .any(|path| path.parent() == Some(destination.as_path()))
            && !clipboard.cut
        {
            self.set_status("같은 폴더에는 붙여넣을 수 없습니다");
            return;
        }
        let (operation, message) = if clipboard.cut {
            (
                FileOperation::Move {
                    items: clipboard.paths.clone(),
                    destination,
                },
                "이동했습니다",
            )
        } else {
            (
                FileOperation::Copy {
                    items: clipboard.paths.clone(),
                    destination,
                },
                "복사했습니다",
            )
        };
        self.start_file_operation(operation, message, None, clipboard.cut);
    }

    fn start_rename(&mut self) {
        let Some(path) = self.selected_path() else {
            self.set_status("이름을 변경할 항목을 선택하세요");
            return;
        };
        self.start_rename_path(path);
    }

    fn start_rename_path(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.dialog = Some(Dialog::Rename { path, name });
    }

    fn start_new_folder(&mut self) {
        let Some(parent) = self
            .panes
            .get(self.active_pane)
            .and_then(|pane| pane.active_tab().history.current().cloned())
        else {
            return;
        };
        self.start_new_folder_at(parent);
    }

    fn start_new_folder_at(&mut self, parent: PathBuf) {
        let name = unique_name(&parent, "새 폴더");
        self.dialog = Some(Dialog::NewFolder { parent, name });
    }

    fn start_delete(&mut self) {
        self.start_delete_items(self.selected_paths(self.active_pane), false);
    }

    fn start_delete_items(&mut self, items: Vec<PathBuf>, permanent: bool) {
        if items.is_empty() {
            self.set_status("삭제할 항목을 선택하세요");
            return;
        }
        self.dialog = Some(Dialog::Delete { items, permanent });
    }

    fn apply_dialog(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Rename { path, name } => {
                let name = name.trim();
                if !is_valid_name(name) {
                    self.set_status("사용할 수 없는 이름입니다");
                    return;
                }
                let Some(parent) = path.parent() else {
                    self.set_status("이름을 변경할 수 없는 경로입니다");
                    return;
                };
                let target = parent.join(name);
                if target == path {
                    return;
                }
                if target.exists() {
                    self.set_status(format!("이미 존재합니다: {}", target.display()));
                    return;
                }
                self.start_file_operation(
                    FileOperation::Rename {
                        path,
                        new_name: name.to_string(),
                    },
                    "이름을 변경했습니다",
                    Some(target),
                    false,
                );
            }
            Dialog::NewFolder { parent, name } => {
                let name = name.trim();
                if !is_valid_name(name) {
                    self.set_status("사용할 수 없는 폴더 이름입니다");
                    return;
                }
                let target = parent.join(name);
                if target.exists() {
                    self.set_status(format!("이미 존재합니다: {}", target.display()));
                    return;
                }
                self.start_file_operation(
                    FileOperation::CreateFolder {
                        parent,
                        name: name.to_string(),
                    },
                    "새 폴더를 만들었습니다",
                    Some(target),
                    false,
                );
            }
            Dialog::Delete { items, permanent } => {
                self.close_preview();
                self.start_file_operation(
                    FileOperation::Delete { items, permanent },
                    if permanent {
                        "영구 삭제했습니다"
                    } else {
                        "휴지통으로 이동했습니다"
                    },
                    None,
                    false,
                );
            }
        }
    }

    fn dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut cancel = false;
        let mut confirm = false;
        let title = match &dialog {
            Dialog::Rename { .. } => "이름 변경",
            Dialog::NewFolder { .. } => "새 폴더",
            Dialog::Delete { .. } => "삭제 확인",
        };

        let mut open = true;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                if ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
                    cancel = true;
                }
                match &mut dialog {
                    Dialog::Rename { path, name } => {
                        ui.label(format!("{}", path.display()));
                        let response =
                            ui.add(egui::TextEdit::singleline(name).desired_width(280.0));
                        response.request_focus();
                        if ui.input(|input| input.key_pressed(Key::Enter)) {
                            confirm = true;
                        }
                    }
                    Dialog::NewFolder { parent, name } => {
                        ui.label(format!("{}", parent.display()));
                        let response =
                            ui.add(egui::TextEdit::singleline(name).desired_width(280.0));
                        response.request_focus();
                        if ui.input(|input| input.key_pressed(Key::Enter)) {
                            confirm = true;
                        }
                    }
                    Dialog::Delete { items, permanent } => {
                        if ui.input(|input| input.key_pressed(Key::Enter)) {
                            confirm = true;
                        }
                        let action = if *permanent {
                            "영구 삭제"
                        } else {
                            "휴지통으로 이동"
                        };
                        let mut preview = String::new();
                        for item in items.iter().take(8) {
                            preview.push_str(&format!("\n{}", item.display()));
                        }
                        if items.len() > 8 {
                            preview.push_str(&format!("\n... 외 {}개", items.len() - 8));
                        }
                        ui.label(format!(
                            "다음 {}개 항목을 {action}할까요?{preview}",
                            items.len()
                        ));
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("취소").clicked() {
                        cancel = true;
                    }
                    if ui.button("확인").clicked() {
                        confirm = true;
                    }
                });
            });

        if !open {
            cancel = true;
        }
        if cancel {
            self.dialog = None;
        } else if confirm {
            self.apply_dialog(dialog);
            self.dialog = None;
        } else {
            self.dialog = Some(dialog);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
                self.close_preview();
            }
            return;
        }

        let mut space = false;
        let mut up = false;
        let mut down = false;
        let mut shift_up = false;
        let mut shift_down = false;
        let mut rename_f2 = false;
        let mut open = false;
        let mut enter = false;
        let mut escape = false;
        let mut parent = false;
        let mut back = false;
        let mut forward = false;
        let mut new_tab = false;
        let mut new_window = false;
        let mut close_tab = false;
        let mut next_tab = false;
        let mut prev_tab = false;
        let mut delete = false;
        let mut permanent_delete = false;
        let mut new_folder = false;
        let mut refresh = false;
        let mut select_all = false;
        let mut copy = false;
        let mut cut = false;
        let mut paste = false;
        let mut properties = false;
        let mut address = false;
        let mut pane_shortcut: Option<usize> = None;

        ctx.input_mut(|input| {
            space = input.consume_key(Modifiers::NONE, Key::Space);
            rename_f2 = input.consume_key(Modifiers::NONE, Key::F2);
            open = input.consume_key(Modifiers::COMMAND, Key::O);
            escape = input.consume_key(Modifiers::NONE, Key::Escape);
            back = input.consume_key(Modifiers::ALT, Key::ArrowLeft);
            forward = input.consume_key(Modifiers::ALT, Key::ArrowRight);
            parent = input.consume_key(Modifiers::ALT, Key::ArrowUp)
                || input.consume_key(Modifiers::NONE, Key::Backspace);
            shift_up = input.consume_key(Modifiers::SHIFT, Key::ArrowUp);
            shift_down = input.consume_key(Modifiers::SHIFT, Key::ArrowDown);
            up = input.consume_key(Modifiers::NONE, Key::ArrowUp);
            down = input.consume_key(Modifiers::NONE, Key::ArrowDown);
            permanent_delete = input.consume_key(Modifiers::SHIFT, Key::Delete);
            delete = input.consume_key(Modifiers::NONE, Key::Delete);
            properties = input.consume_key(Modifiers::ALT, Key::Enter);
            enter = input.consume_key(Modifiers::NONE, Key::Enter);
            new_folder = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::N);
            new_window = input.consume_key(Modifiers::COMMAND, Key::N);
            new_tab = input.consume_key(Modifiers::COMMAND, Key::T);
            close_tab = input.consume_key(Modifiers::COMMAND, Key::W);
            prev_tab = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab);
            next_tab = input.consume_key(Modifiers::COMMAND, Key::Tab);
            refresh = input.consume_key(Modifiers::NONE, Key::F5);
            select_all = input.consume_key(Modifiers::COMMAND, Key::A);
            copy = input.consume_key(Modifiers::COMMAND, Key::C);
            cut = input.consume_key(Modifiers::COMMAND, Key::X);
            paste = input.consume_key(Modifiers::COMMAND, Key::V);
            address = input.consume_key(Modifiers::COMMAND, Key::L)
                || input.consume_key(Modifiers::ALT, Key::D);
            for (key, index) in [
                (Key::Num1, 0),
                (Key::Num2, 1),
                (Key::Num3, 2),
                (Key::Num4, 3),
            ] {
                if input.consume_key(Modifiers::COMMAND, key) {
                    pane_shortcut = Some(index);
                }
            }
        });

        if space {
            self.toggle_preview(ctx);
        }
        if escape {
            self.close_preview();
        }
        if up {
            self.move_selection(ctx, -1, false);
        }
        if down {
            self.move_selection(ctx, 1, false);
        }
        if shift_up {
            self.move_selection(ctx, -1, true);
        }
        if shift_down {
            self.move_selection(ctx, 1, true);
        }
        if enter || open {
            self.open_selected(ctx);
        }
        if rename_f2 {
            self.start_rename();
        }
        if parent {
            self.go_up(ctx, self.active_pane);
        }
        if back {
            self.go_back(ctx, self.active_pane);
        }
        if forward {
            self.go_forward(ctx, self.active_pane);
        }
        if new_tab {
            self.new_tab(self.active_pane);
        }
        if new_window {
            let path = self
                .panes
                .get(self.active_pane)
                .and_then(|pane| pane.active_tab().history.current().cloned())
                .unwrap_or_else(winfinder_core::state::default_dir);
            self.new_window(path);
        }
        if new_folder {
            self.start_new_folder();
        }
        if delete {
            self.start_delete_items(self.selected_paths(self.active_pane), false);
        }
        if permanent_delete {
            self.start_delete_items(self.selected_paths(self.active_pane), true);
        }
        if refresh {
            self.reload_active();
        }
        if select_all {
            if let Some(pane) = self.panes.get_mut(self.active_pane) {
                let tab = pane.active_tab_mut();
                let ordered = tab.ordered_paths();
                tab.selection.select_all(&ordered);
            }
            self.dirty = true;
            self.after_selection_changed(ctx);
        }
        if copy {
            self.copy_selection(false);
        }
        if cut {
            self.copy_selection(true);
        }
        if paste
            && let Some(path) = self
                .panes
                .get(self.active_pane)
                .and_then(|pane| pane.active_tab().history.current().cloned())
        {
            self.paste_into(path);
        }
        if properties {
            let paths = self.selected_paths(self.active_pane);
            if !paths.is_empty()
                && let Err(err) = winfinder_shell::show_properties(&paths)
            {
                self.set_status(err);
            }
        }
        if address && let Some(pane) = self.panes.get_mut(self.active_pane) {
            pane.active_tab_mut().start_address_edit();
        }
        if close_tab {
            let index = self.panes[self.active_pane].active;
            self.close_tab(self.active_pane, index);
        }
        if next_tab || prev_tab {
            let pane = &self.panes[self.active_pane];
            let count = pane.tabs.len();
            let index = if next_tab {
                (pane.active + 1) % count
            } else {
                (pane.active + count - 1) % count
            };
            self.switch_tab(self.active_pane, index);
        }
        if let Some(index) = pane_shortcut
            && index < self.preset.pane_count()
        {
            self.active_pane = index;
        }
    }

    fn apply_action(&mut self, ctx: &egui::Context, pane_idx: usize, action: Action) {
        match action {
            Action::Select(path) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().selection.select_only(path);
                }
                self.dirty = true;
                self.after_selection_changed(ctx);
            }
            Action::ToggleSelect(path) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().selection.toggle(path);
                }
                self.dirty = true;
                self.after_selection_changed(ctx);
            }
            Action::SelectRange(path, additive) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    let ordered = tab.ordered_paths();
                    if let Some(index) = ordered.iter().position(|item| item == &path) {
                        tab.selection.select_range(&ordered, index, additive);
                    }
                }
                self.dirty = true;
                self.after_selection_changed(ctx);
            }
            Action::Open(path) => self.open_path(ctx, pane_idx, path),
            Action::OpenInNewTab(path) => self.new_tab_at(pane_idx, path),
            Action::OpenInOtherPane(path) => {
                let count = self.preset.pane_count();
                if count < 2 {
                    self.set_status("다른 패널이 없습니다");
                } else {
                    let other = (pane_idx + 1) % count;
                    self.navigate(ctx, other, path);
                }
            }
            Action::OpenInNewWindow(path) => self.new_window(path),
            Action::Back => self.go_back(ctx, pane_idx),
            Action::Forward => self.go_forward(ctx, pane_idx),
            Action::Up => self.go_up(ctx, pane_idx),
            Action::NewTab => self.new_tab(pane_idx),
            Action::CloseTab(index) => self.close_tab(pane_idx, index),
            Action::SwitchTab(index) => self.switch_tab(pane_idx, index),
            Action::Navigate(path) => self.navigate(ctx, pane_idx, path),
            Action::StartAddressEdit => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().start_address_edit();
                }
            }
            Action::SetView(mode) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().view_mode = mode;
                }
                self.dirty = true;
            }
            Action::SetHidden(show) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    tab.show_hidden = show;
                    tab.rebuild_view();
                }
                self.dirty = true;
            }
            Action::SetShowExtensions(show) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().show_extensions = show;
                }
                self.dirty = true;
            }
            Action::ColumnsChanged => {
                self.dirty = true;
            }
            Action::SetSort(key) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    if tab.sort == key {
                        tab.sort_desc = !tab.sort_desc;
                    } else {
                        tab.sort = key;
                        tab.sort_desc = false;
                    }
                    tab.apply_sort();
                }
                self.dirty = true;
            }
            Action::SetSortDesc(desc) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    tab.sort_desc = desc;
                    tab.apply_sort();
                }
                self.dirty = true;
            }
            Action::Refresh => {
                self.reload_pane(pane_idx);
                self.set_status("새로 고침했습니다");
            }
            Action::NewFolder(parent) => {
                self.active_pane = pane_idx;
                self.start_new_folder_at(parent);
            }
            Action::Rename(path) => {
                self.active_pane = pane_idx;
                self.start_rename_path(path);
            }
            Action::Delete(items, permanent) => {
                self.active_pane = pane_idx;
                self.start_delete_items(items, permanent);
            }
            Action::Preview(path) => {
                self.active_pane = pane_idx;
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    pane.active_tab_mut().selection.select_only(path);
                }
                self.dirty = true;
                self.show_preview(ctx);
            }
            Action::CopyPath(paths) => {
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ctx.copy_text(text);
                    self.set_status("경로를 복사했습니다");
                }
            }
            Action::Copy => {
                self.active_pane = pane_idx;
                self.copy_selection(false);
            }
            Action::Cut => {
                self.active_pane = pane_idx;
                self.copy_selection(true);
            }
            Action::Paste(destination) => {
                self.active_pane = pane_idx;
                self.paste_into(destination);
            }
            Action::Properties(paths) => {
                if !paths.is_empty()
                    && let Err(err) = winfinder_shell::show_properties(&paths)
                {
                    self.set_status(err);
                }
            }
            Action::OpenWith(path) => {
                if let Err(err) = winfinder_shell::open_with(&path) {
                    self.set_status(err);
                }
            }
            Action::ShellMenu { paths, background } => {
                if let Err(err) =
                    winfinder_shell::show_context_menu(&paths, background.as_deref(), (0, 0))
                {
                    self.set_status(err);
                }
                self.reload_pane(pane_idx);
            }
            Action::StartDrag(paths) => {
                if let Err(err) = winfinder_shell::start_drag(&paths) {
                    self.set_status(err);
                }
                self.reload_active();
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("winFinder").strong().size(15.0));
            ui.add_space(3.0);
            ui.separator();

            if icon_button(ui, Icon::NewFolder, "새 폴더 (Ctrl/Cmd+Shift+N)", false).clicked() {
                self.start_new_folder();
            }
            if icon_button(ui, Icon::Rename, "이름 변경 (F2)", false).clicked() {
                self.start_rename();
            }
            if icon_button(ui, Icon::Delete, "삭제 (Delete)", false).clicked() {
                self.start_delete();
            }

            ui.separator();
            for preset in LayoutPreset::all() {
                let icon = match preset {
                    LayoutPreset::One => Icon::LayoutOne,
                    LayoutPreset::OneByTwo => Icon::LayoutTwo,
                    LayoutPreset::TwoByTwo => Icon::LayoutFour,
                };
                if icon_button(ui, icon, preset.label(), self.preset == preset).clicked() {
                    self.preset = preset;
                    self.active_pane = self.active_pane.min(preset.pane_count() - 1);
                    self.dirty = true;
                }
            }

            ui.separator();
            if icon_button(ui, Icon::Preview, "미리보기 (Space)", self.preview.open).clicked() {
                self.toggle_preview(ui.ctx());
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (icon, tooltip) = match self.theme {
                    Theme::Light => (Icon::Moon, "다크 테마로 전환"),
                    Theme::Dark => (Icon::Sun, "라이트 테마로 전환"),
                };
                if icon_button(ui, icon, tooltip, false).clicked() {
                    self.toggle_theme(ui.ctx());
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("즐겨찾기").strong());
        for (label, path) in quick_access() {
            if ui.selectable_label(false, label).clicked() {
                let active = self.active_pane;
                self.navigate(ui.ctx(), active, path);
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new("드라이브").strong());
        for root in roots() {
            let label = root.display().to_string();
            if ui.selectable_label(false, label).clicked() {
                let active = self.active_pane;
                self.navigate(ui.ctx(), active, root);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let palette = Palette::from_visuals(ui.visuals());
        ui.horizontal(|ui| {
            if let Some((message, at)) = &self.status {
                if at.elapsed() < STATUS_TTL {
                    ui.label(RichText::new(message).color(palette.accent));
                } else {
                    ui.label("");
                }
            } else {
                let pane = &self.panes[self.active_pane];
                let tab = pane.active_tab();
                let path = tab
                    .history
                    .current()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                ui.label(RichText::new(path).color(palette.text_weak));
                if let Some(error) = &tab.error {
                    ui.colored_label(palette.danger, error);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tab = self.panes[self.active_pane].active_tab();
                if self.operations > 0 {
                    ui.spinner();
                }
                let selected = tab.selection.len();
                let selection = if selected > 0 {
                    format!(" · {selected}개 선택")
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!(
                        "{}개 항목{selection} · 탭 {}/{}",
                        tab.view.len(),
                        self.panes[self.active_pane].active + 1,
                        tab_count(&self.panes[self.active_pane])
                    ))
                    .color(palette.text_weak)
                    .size(11.0),
                );
            });
        });
    }

    fn grid(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let cols = self.preset.columns();
        let rows = self.preset.rows();
        let (column_rects, x_handle) =
            split_axis(rect, cols, PANE_GAP, self.split_x, MIN_PANE_WIDTH, true);
        let (row_rects, y_handle) =
            split_axis(rect, rows, PANE_GAP, self.split_y, MIN_PANE_HEIGHT, false);
        let mut split_changed = false;

        if let Some(handle_rect) = x_handle {
            let response = ui.interact(
                handle_rect,
                ui.make_persistent_id("winfinder-split-x"),
                Sense::drag(),
            );
            if update_split_ratio(
                &mut self.split_x,
                &response,
                rect,
                PANE_GAP,
                MIN_PANE_WIDTH,
                true,
            ) {
                split_changed = true;
            }
            paint_split_handle(ui, &response, true);
        }
        if let Some(handle_rect) = y_handle {
            let response = ui.interact(
                handle_rect,
                ui.make_persistent_id("winfinder-split-y"),
                Sense::drag(),
            );
            if update_split_ratio(
                &mut self.split_y,
                &response,
                rect,
                PANE_GAP,
                MIN_PANE_HEIGHT,
                false,
            ) {
                split_changed = true;
            }
            paint_split_handle(ui, &response, false);
        }
        if split_changed {
            self.dirty = true;
        }

        for index in 0..self.preset.pane_count() {
            let col = index % cols;
            let row = index / cols;
            let cell = Rect::from_min_max(
                pos2(column_rects[col].left(), row_rects[row].top()),
                pos2(column_rects[col].right(), row_rects[row].bottom()),
            );
            let active = index == self.active_pane;
            let palette = Palette::from_visuals(ui.visuals());
            ui.painter().rect_filled(
                cell,
                6.0,
                if active {
                    palette.pane_active
                } else {
                    palette.pane_inactive
                },
            );
            ui.painter().rect_stroke(
                cell,
                6.0,
                Stroke::new(
                    if active { 1.8 } else { 1.0 },
                    if active {
                        palette.accent
                    } else {
                        palette.pane_border
                    },
                ),
                StrokeKind::Inside,
            );

            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(cell.shrink(5.0))
                    .layout(Layout::top_down(Align::Min)),
            );
            self.pane_ui(&mut child, index);
        }
    }

    fn pane_ui(&mut self, ui: &mut Ui, pane_index: usize) {
        let clicked_inside = ui.ctx().pointer_latest_pos().is_some_and(|pos| {
            ui.max_rect().contains(pos) && ui.ctx().input(|input| input.pointer.any_click())
        });
        if clicked_inside {
            self.active_pane = pane_index;
        }

        let drop_hover = self.drop_hover.clone();
        let clipboard_available = self.clipboard.is_some() || cfg!(windows);
        let (panes, thumbnails) = (&mut self.panes, &mut self.thumbnails);
        let actions = pane_contents(
            ui,
            &mut panes[pane_index],
            pane_index,
            drop_hover.as_ref(),
            clipboard_available,
            thumbnails,
        );
        let ctx = ui.ctx().clone();
        for action in actions {
            self.apply_action(&ctx, pane_index, action);
        }
    }

    #[allow(deprecated)]
    fn preview_window(&mut self, ctx: &egui::Context) {
        let mut open = self.preview.open;
        let title = self
            .preview
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "미리보기".to_string());

        egui::Window::new(title)
            .id(Id::new("winfinder-preview"))
            .open(&mut open)
            .default_size([760.0, 560.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                let path = self.preview.path.clone();
                let cache = &mut self.cm_cache;
                let PreviewState {
                    content,
                    texture,
                    audio,
                    video,
                    video_error,
                    ..
                } = &mut self.preview;

                let runtime_error = video.as_ref().and_then(|player| player.error());
                if let Some(player) = video.as_ref()
                    && let Some(frame) = player.take_frame()
                {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    );
                    if let Some(texture) = texture {
                        texture.set(image, TextureOptions::LINEAR);
                    } else {
                        *texture = Some(ui.ctx().load_texture(
                            "preview:video",
                            image,
                            TextureOptions::LINEAR,
                        ));
                    }
                }

                if let Some(path) = &path {
                    ui.label(
                        RichText::new(path.display().to_string())
                            .color(Color32::from_gray(140))
                            .size(11.0),
                    );
                    ui.separator();
                }

                match content.as_ref() {
                    Some(Preview::Text {
                        text,
                        truncated,
                        encoding,
                    }) => {
                        if *truncated {
                            ui.label(
                                RichText::new("파일이 커서 앞부분만 표시합니다")
                                    .color(Color32::from_rgb(230, 180, 90)),
                            );
                        }
                        ui.label(
                            RichText::new(format!("encoding: {encoding}"))
                                .color(Color32::from_gray(120))
                                .size(10.0),
                        );
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text.as_str()).monospace().size(12.0),
                                )
                                .selectable(true),
                            );
                        });
                    }
                    Some(Preview::Markdown(markdown)) => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui_commonmark::CommonMarkViewer::new().show(ui, cache, markdown);
                        });
                    }
                    Some(Preview::Image {
                        width,
                        height,
                        rgba: _,
                    })
                    | Some(Preview::Pdf { width, height, .. }) => {
                        if let Some(texture) = texture {
                            let available = ui.available_size();
                            let scale = (available.x / *width as f32)
                                .min(available.y / *height as f32)
                                .min(1.0);
                            ui.add(
                                egui::Image::from_texture(&*texture)
                                    .max_size(vec2(*width as f32 * scale, *height as f32 * scale))
                                    .maintain_aspect_ratio(true),
                            );
                        } else {
                            ui.label("이미지를 표시할 수 없습니다");
                        }
                    }
                    Some(Preview::Audio { info }) => {
                        ui.label(info);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if let Some(player) = audio {
                                if ui
                                    .button(if player.is_paused() {
                                        "재생"
                                    } else {
                                        "일시정지"
                                    })
                                    .clicked()
                                {
                                    player.toggle();
                                }
                                if ui.button("정지").clicked() {
                                    player.stop();
                                }
                            } else {
                                ui.label("오디오 장치를 사용할 수 없습니다");
                            }
                        });
                    }
                    Some(Preview::Video { info }) => {
                        ui.label(info);
                        if let Some(error) = video_error.as_ref().or(runtime_error.as_ref()) {
                            ui.colored_label(Color32::from_rgb(240, 110, 110), error);
                        }
                        if let Some(texture) = texture {
                            let available = ui.available_size();
                            let width = texture.size()[0] as f32;
                            let height = texture.size()[1] as f32;
                            let scale = (available.x / width).min(available.y / height).min(1.0);
                            ui.add(
                                egui::Image::from_texture(&*texture)
                                    .max_size(vec2(width * scale, height * scale))
                                    .maintain_aspect_ratio(true),
                            );
                        } else if let Some(player) = video.as_ref()
                            && !player.is_finished()
                        {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("동영상을 준비하는 중입니다");
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if let Some(player) = video {
                                if player.is_finished() {
                                    ui.label("재생 완료");
                                } else if ui
                                    .button(if player.is_paused() {
                                        "재생"
                                    } else {
                                        "일시정지"
                                    })
                                    .clicked()
                                {
                                    player.toggle();
                                }
                            }
                            if let Some(path) = &path
                                && ui.button("기본 앱으로 열기").clicked()
                            {
                                let _ = open::that(path);
                            }
                        });
                    }
                    Some(Preview::Info { kind, info }) => {
                        ui.label(RichText::new(kind).strong());
                        ui.label(info);
                    }
                    Some(Preview::Error(error)) => {
                        ui.colored_label(Color32::from_rgb(240, 110, 110), error);
                    }
                    None => {
                        ui.spinner();
                    }
                }
            });

        self.preview.open = open;
        if !open {
            self.preview.audio = None;
            self.preview.video = None;
        } else if let Some(player) = self.preview.video.as_ref()
            && !player.is_finished()
            && !player.is_paused()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn build_session(&self) -> winfinder_core::Session {
        let mut session = winfinder_core::Session {
            version: winfinder_core::state::STATE_VERSION,
            window: self.window_state.clone(),
            theme: self.theme,
            preset: self.preset,
            split_x: self.split_x,
            split_y: self.split_y,
            active_pane: self.active_pane,
            panes: Vec::new(),
        };
        for pane in &self.panes {
            let tabs: Vec<TabState> = pane
                .tabs
                .iter()
                .take(winfinder_core::MAX_TABS_PER_PANE)
                .map(Tab::to_state)
                .collect();
            session.panes.push(winfinder_core::PaneState {
                tabs,
                active_tab: pane.active.min(pane.tabs.len().saturating_sub(1)),
            });
        }
        session.normalize();
        session
    }

    fn save_now(&mut self) {
        if !self.save_enabled {
            self.dirty = false;
            self.last_save = Instant::now();
            return;
        }
        match self.build_session().save() {
            Ok(()) => {
                self.dirty = false;
                self.last_save = Instant::now();
            }
            Err(err) => {
                self.set_status(format!("상태 저장 실패: {err}"));
                self.last_save = Instant::now();
            }
        }
    }

    fn update_window_state(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            let viewport = input.viewport();
            if let Some(rect) = viewport.inner_rect {
                self.window_state.width = rect.width();
                self.window_state.height = rect.height();
            }
            if let Some(rect) = viewport.outer_rect {
                self.window_state.position = Some([rect.min.x, rect.min.y]);
            }
            if let Some(maximized) = viewport.maximized {
                self.window_state.maximized = maximized;
            }
        });
    }
}

impl eframe::App for WinFinderApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.update_window_state(&ctx);
        self.poll_events(&ctx);
        self.handle_shortcuts(&ctx);
        if ctx.input(|input| input.focused)
            && self.dialog.is_none()
            && self.operations == 0
            && self.last_auto_refresh.elapsed() >= AUTO_REFRESH
        {
            self.last_auto_refresh = Instant::now();
            if let Some(path) = self
                .panes
                .get(self.active_pane)
                .and_then(|pane| pane.active_tab().history.current().cloned())
                && !self.panes[self.active_pane].active_tab().loading
            {
                self.request_load(self.active_pane, path, LoadKind::Refresh);
            }
        }

        egui::Panel::top("winfinder-top").show(ui, |ui| {
            self.top_bar(ui);
        });
        egui::Panel::left("winfinder-sidebar")
            .resizable(true)
            .default_size(190.0)
            .show(ui, |ui| {
                self.sidebar(ui);
            });
        egui::Panel::bottom("winfinder-status").show(ui, |ui| {
            self.status_bar(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.grid(ui);
        });

        if self.preview.open {
            self.preview_window(&ctx);
        }
        if self.dialog.is_some() {
            self.dialog_window(&ctx);
        }

        if self.dirty && self.last_save.elapsed() >= SAVE_DEBOUNCE {
            self.save_now();
        }
        if self.status.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_now();
        winfinder_shell::shutdown();
    }
}

fn scroll_to_show_row(index: usize, scroll: f32, viewport_height: f32, row_pitch: f32) -> f32 {
    let row_top = index as f32 * row_pitch;
    let row_bottom = row_top + row_pitch;
    let viewport_height = viewport_height.max(row_pitch);

    if row_top < scroll {
        row_top
    } else if row_bottom > scroll + viewport_height {
        row_bottom - viewport_height
    } else {
        scroll
    }
}

fn split_axis(
    rect: Rect,
    count: usize,
    gap: f32,
    ratio: f32,
    min_extent: f32,
    horizontal: bool,
) -> (Vec<Rect>, Option<Rect>) {
    if count < 2 {
        return (vec![rect], None);
    }

    let extent = if horizontal {
        rect.width()
    } else {
        rect.height()
    };
    let available = (extent - gap).max(1.0);
    let min_ratio = (min_extent / available).clamp(0.0, 0.45);
    let ratio = ratio.clamp(min_ratio, 1.0 - min_ratio);
    let first_extent = available * ratio;

    let (first, second, handle) = if horizontal {
        let split = rect.left() + first_extent;
        (
            Rect::from_min_max(rect.min, pos2(split, rect.bottom())),
            Rect::from_min_max(pos2(split + gap, rect.top()), rect.max),
            Rect::from_min_max(pos2(split, rect.top()), pos2(split + gap, rect.bottom())),
        )
    } else {
        let split = rect.top() + first_extent;
        (
            Rect::from_min_max(rect.min, pos2(rect.right(), split)),
            Rect::from_min_max(pos2(rect.left(), split + gap), rect.max),
            Rect::from_min_max(pos2(rect.left(), split), pos2(rect.right(), split + gap)),
        )
    };

    (vec![first, second], Some(handle))
}

fn update_split_ratio(
    ratio: &mut f32,
    response: &egui::Response,
    rect: Rect,
    gap: f32,
    min_extent: f32,
    horizontal: bool,
) -> bool {
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let extent = if horizontal {
            rect.width()
        } else {
            rect.height()
        };
        let available = (extent - gap).max(1.0);
        let min_ratio = (min_extent / available).clamp(0.0, 0.45);
        let offset = if horizontal {
            pointer.x - rect.left()
        } else {
            pointer.y - rect.top()
        };
        *ratio = (offset / available).clamp(min_ratio, 1.0 - min_ratio);
        true
    } else {
        false
    }
}

fn paint_split_handle(ui: &Ui, response: &egui::Response, horizontal: bool) {
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if horizontal {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        });
    }

    let palette = Palette::from_visuals(ui.visuals());
    let color = if response.dragged() {
        palette.accent
    } else if response.hovered() {
        palette.splitter_hover
    } else {
        palette.splitter
    };
    let line = Rect::from_center_size(
        response.rect.center(),
        if horizontal {
            vec2(2.0, response.rect.height())
        } else {
            vec2(response.rect.width(), 2.0)
        },
    );
    ui.painter().rect_filled(line, 1.0, color);
}

fn pane_contents(
    ui: &mut Ui,
    pane: &mut Pane,
    pane_index: usize,
    drop_hover: Option<&DropHover>,
    clipboard_available: bool,
    thumbnails: &mut ThumbnailStore,
) -> Vec<Action> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        let mut switch = None;
        let mut close = None;
        let mut add = false;
        for (index, tab) in pane.tabs.iter().enumerate() {
            ui.push_id(("tab", pane_index, index), |ui| {
                if ui
                    .selectable_label(index == pane.active, tab.display_name())
                    .clicked()
                {
                    switch = Some(index);
                }
                if pane.tabs.len() > 1 && ui.small_button("×").clicked() {
                    close = Some(index);
                }
            });
        }
        if ui
            .small_button("+")
            .on_hover_text(format!(
                "새 탭 (최대 {}개)",
                winfinder_core::MAX_TABS_PER_PANE
            ))
            .clicked()
        {
            add = true;
        }
        if let Some(index) = switch {
            actions.push(Action::SwitchTab(index));
        }
        if let Some(index) = close {
            actions.push(Action::CloseTab(index));
        }
        if add {
            actions.push(Action::NewTab);
        }
    });

    let current_path = pane
        .active_tab()
        .history
        .current()
        .cloned()
        .unwrap_or_default();
    let can_back = pane.active_tab().history.can_back();
    let can_forward = pane.active_tab().history.can_forward();
    let active_index = pane.active;
    let tab = pane.active_tab_mut();
    let palette = Palette::from_visuals(ui.visuals());

    ui.spacing_mut().item_spacing.x = 3.0;
    ui.horizontal(|ui| {
        if icon_button_enabled(ui, can_back, Icon::Back, "뒤로 (Alt+←)").clicked() {
            actions.push(Action::Back);
        }
        if icon_button_enabled(ui, can_forward, Icon::Forward, "앞으로 (Alt+→)").clicked() {
            actions.push(Action::Forward);
        }
        if icon_button(ui, Icon::Up, "상위 폴더 (Backspace, Alt+↑)", false).clicked() {
            actions.push(Action::Up);
        }
        if icon_button(ui, Icon::Refresh, "새로 고침 (F5)", false).clicked() {
            actions.push(Action::Refresh);
        }

        let view_mode = tab.view_mode;
        let sort = tab.sort;
        let sort_desc = tab.sort_desc;
        let show_hidden = tab.show_hidden;
        let show_extensions = tab.show_extensions;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let response = icon_button(ui, Icon::ViewOptions, "보기 옵션", false);
            egui::Popup::menu(&response)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    view_options(
                        ui,
                        &mut actions,
                        view_mode,
                        sort,
                        sort_desc,
                        show_hidden,
                        show_extensions,
                    );
                });
        });
    });

    if tab.address_editing {
        let available = ui.available_width();
        let response = ui.add(
            egui::TextEdit::singleline(&mut tab.address_buffer)
                .desired_width(available)
                .font(egui::TextStyle::Monospace)
                .hint_text("경로 입력"),
        );
        if tab.address_focus {
            response.request_focus();
            tab.address_focus = false;
        }
        if ui.input(|input| input.key_pressed(Key::Enter)) {
            let path = PathBuf::from(tab.address_buffer.trim());
            if !path.as_os_str().is_empty() {
                actions.push(Action::Navigate(path));
            }
            tab.address_editing = false;
        } else if ui.input(|input| input.key_pressed(Key::Escape)) {
            tab.address_editing = false;
        }
    } else {
        egui::Frame::new()
            .fill(palette.path_bg)
            .stroke(Stroke::new(1.0, palette.pane_border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let segments = breadcrumb_segments(&current_path);
                    for (index, (label, path)) in segments.iter().enumerate() {
                        if index > 0 {
                            ui.label(RichText::new("›").color(palette.text_weak));
                        }
                        let response = ui.add(
                            egui::Label::new(RichText::new(label).size(11.0).color(
                                if index + 1 == segments.len() {
                                    palette.text
                                } else {
                                    palette.text_weak
                                },
                            ))
                            .sense(Sense::click()),
                        );
                        if response.clicked() {
                            actions.push(Action::Navigate(path.clone()));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("⌨")
                            .on_hover_text("경로 직접 입력 (Ctrl+L)")
                            .clicked()
                        {
                            actions.push(Action::StartAddressEdit);
                        }
                    });
                });
            });
    }

    if let Some(error) = &tab.error {
        ui.colored_label(palette.danger, error);
    }

    if matches!(tab.view_mode, ViewMode::Details | ViewMode::List) {
        let (sort_key, resized) =
            draw_header(ui, &palette, &mut tab.columns, tab.sort, tab.sort_desc);
        if let Some(key) = sort_key {
            actions.push(Action::SetSort(key));
        }
        if resized {
            actions.push(Action::ColumnsChanged);
        }
    }

    let files_rect = ui.available_rect_before_wrap();
    tab.content_rect = files_rect;
    let background = ui.interact(
        files_rect,
        ui.make_persistent_id(("files-background", pane_index, active_index)),
        Sense::click(),
    );
    let menu_path = current_path.clone();
    egui::Popup::context_menu(&background)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            background_menu(
                ui,
                &mut actions,
                &menu_path,
                clipboard_available,
                tab.view_mode,
                tab.sort,
                tab.sort_desc,
                tab.show_hidden,
                tab.show_extensions,
            );
        });

    tab.row_pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
    let mut hit_rows = Vec::new();
    let mut action = None;
    let view_len = tab.view.len();
    let view_mode = tab.view_mode;

    if view_mode == ViewMode::Icons || view_mode == ViewMode::Tiles {
        let tile = view_mode == ViewMode::Tiles;
        let output = egui::ScrollArea::vertical()
            .id_salt(("files-grid", pane_index, active_index))
            .auto_shrink([false, false])
            .vertical_scroll_offset(tab.scroll)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if view_len == 0 {
                    ui.label(RichText::new("빈 폴더").color(palette.text_weak));
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for row in 0..view_len {
                        let entry = tab.entries[tab.view[row]].clone();
                        let path = entry.path.clone();
                        let is_dir = entry.is_dir;
                        let is_selected = tab.selection.contains(&path);
                        let hovered = drop_hover
                            .is_some_and(|hover| hover.pane == pane_index && hover.path == path);
                        let response = draw_grid_cell(
                            ui,
                            &entry,
                            &palette,
                            tab.show_extensions,
                            GridCellState {
                                selected: is_selected,
                                tile,
                                drop_hover: hovered,
                            },
                            thumbnails,
                        );
                        hit_rows.push((response.rect, path.clone(), is_dir));
                        let menu_selection = if is_selected {
                            tab.selection.paths()
                        } else {
                            vec![path.clone()]
                        };
                        let menu_path = path.clone();
                        response.context_menu(|ui| {
                            item_menu(
                                ui,
                                &mut actions,
                                &menu_path,
                                is_dir,
                                menu_selection.clone(),
                                tab.view_mode,
                                tab.sort,
                                tab.sort_desc,
                                tab.show_hidden,
                                tab.show_extensions,
                            );
                        });
                        if response.double_clicked() {
                            action = Some(Action::Open(path.clone()));
                        } else if response.clicked() {
                            let modifiers = ui.input(|input| input.modifiers);
                            if modifiers.shift {
                                action = Some(Action::SelectRange(path.clone(), modifiers.command));
                            } else if modifiers.command {
                                action = Some(Action::ToggleSelect(path.clone()));
                            } else {
                                action = Some(Action::Select(path.clone()));
                            }
                        }
                        if response.secondary_clicked() && !is_selected {
                            action = Some(Action::Select(path.clone()));
                        }
                        if response.drag_started() {
                            let paths = if is_selected {
                                tab.selection.paths()
                            } else {
                                vec![path.clone()]
                            };
                            actions.push(Action::StartDrag(paths));
                        }
                    }
                });
            });
        tab.viewport_height = output.inner_rect.height();
        tab.scroll = output.state.clamped_offset().y;
    } else {
        let output = egui::ScrollArea::vertical()
            .id_salt(("files", pane_index, active_index))
            .auto_shrink([false, false])
            .vertical_scroll_offset(tab.scroll)
            .show_rows(ui, ROW_HEIGHT, view_len, |ui, range| {
                for row in range {
                    let entry = tab.entries[tab.view[row]].clone();
                    let path = entry.path.clone();
                    let is_dir = entry.is_dir;
                    let is_selected = tab.selection.contains(&path);
                    let hovered = drop_hover
                        .is_some_and(|hover| hover.pane == pane_index && hover.path == path);
                    let response = draw_row(
                        ui,
                        &entry,
                        is_selected,
                        &palette,
                        &tab.columns,
                        tab.show_extensions,
                        hovered,
                    );
                    hit_rows.push((response.rect, path.clone(), is_dir));
                    let menu_selection = if is_selected {
                        tab.selection.paths()
                    } else {
                        vec![path.clone()]
                    };
                    let menu_path = path.clone();
                    response.context_menu(|ui| {
                        item_menu(
                            ui,
                            &mut actions,
                            &menu_path,
                            is_dir,
                            menu_selection.clone(),
                            tab.view_mode,
                            tab.sort,
                            tab.sort_desc,
                            tab.show_hidden,
                            tab.show_extensions,
                        );
                    });
                    if response.double_clicked() {
                        action = Some(Action::Open(path.clone()));
                    } else if response.clicked() {
                        let modifiers = ui.input(|input| input.modifiers);
                        if modifiers.shift {
                            action = Some(Action::SelectRange(path.clone(), modifiers.command));
                        } else if modifiers.command {
                            action = Some(Action::ToggleSelect(path.clone()));
                        } else {
                            action = Some(Action::Select(path.clone()));
                        }
                    }
                    if response.secondary_clicked() && !is_selected {
                        action = Some(Action::Select(path.clone()));
                    }
                    if response.drag_started() {
                        let paths = if is_selected {
                            tab.selection.paths()
                        } else {
                            vec![path.clone()]
                        };
                        actions.push(Action::StartDrag(paths));
                    }
                }
            });
        tab.viewport_height = output.inner_rect.height();
        tab.scroll = output.state.clamped_offset().y;
    }

    tab.hit_rows = hit_rows;
    if let Some(action) = action {
        actions.push(action);
    }

    actions
}

#[allow(clippy::too_many_arguments)]
fn background_menu(
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    current_path: &Path,
    clipboard_available: bool,
    view_mode: ViewMode,
    sort: SortKey,
    sort_desc: bool,
    show_hidden: bool,
    show_extensions: bool,
) {
    ui.set_min_width(220.0);
    if ui
        .add_enabled(clipboard_available, egui::Button::new("붙여넣기"))
        .clicked()
    {
        actions.push(Action::Paste(current_path.to_path_buf()));
        ui.close();
    }
    if ui.button("새 폴더").clicked() {
        actions.push(Action::NewFolder(current_path.to_path_buf()));
        ui.close();
    }
    if ui.button("새로 고침").clicked() {
        actions.push(Action::Refresh);
        ui.close();
    }
    ui.separator();
    ui.menu_button(format!("보기: {}", view_mode.label()), |ui| {
        for mode in ViewMode::all() {
            if ui
                .selectable_label(view_mode == mode, mode.label())
                .clicked()
            {
                actions.push(Action::SetView(mode));
                ui.close();
            }
        }
    });
    sort_menu(ui, actions, sort, sort_desc);
    ui.separator();
    let mut hidden = show_hidden;
    if ui.checkbox(&mut hidden, "숨김 항목 표시").changed() {
        actions.push(Action::SetHidden(hidden));
    }
    let mut extensions = show_extensions;
    if ui.checkbox(&mut extensions, "파일 확장명 표시").changed() {
        actions.push(Action::SetShowExtensions(extensions));
    }
    ui.separator();
    if ui.button("경로 복사").clicked() {
        actions.push(Action::CopyPath(vec![current_path.to_path_buf()]));
        ui.close();
    }
    if ui.button("속성").clicked() {
        actions.push(Action::Properties(vec![current_path.to_path_buf()]));
        ui.close();
    }
    ui.separator();
    if ui.button("추가 옵션...").clicked() {
        actions.push(Action::ShellMenu {
            paths: Vec::new(),
            background: Some(current_path.to_path_buf()),
        });
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
fn item_menu(
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    path: &Path,
    is_dir: bool,
    selection: Vec<PathBuf>,
    view_mode: ViewMode,
    sort: SortKey,
    sort_desc: bool,
    show_hidden: bool,
    show_extensions: bool,
) {
    ui.set_min_width(230.0);
    if selection.len() > 1 {
        ui.label(RichText::new(format!("{}개 항목", selection.len())).weak());
        ui.separator();
    }
    if ui.button("열기").clicked() {
        actions.push(Action::Open(path.to_path_buf()));
        ui.close();
    }
    if is_dir {
        if ui.button("새 탭에서 열기").clicked() {
            actions.push(Action::OpenInNewTab(path.to_path_buf()));
            ui.close();
        }
        if ui.button("새 창에서 열기").clicked() {
            actions.push(Action::OpenInNewWindow(path.to_path_buf()));
            ui.close();
        }
        if ui.button("반대 패널에서 열기").clicked() {
            actions.push(Action::OpenInOtherPane(path.to_path_buf()));
            ui.close();
        }
    } else if ui.button("연결 프로그램...").clicked() {
        actions.push(Action::OpenWith(path.to_path_buf()));
        ui.close();
    }
    if !is_dir && ui.button("미리보기").clicked() {
        actions.push(Action::Preview(path.to_path_buf()));
        ui.close();
    }
    ui.separator();
    if ui.button("잘라내기").clicked() {
        actions.push(Action::Cut);
        ui.close();
    }
    if ui.button("복사").clicked() {
        actions.push(Action::Copy);
        ui.close();
    }
    ui.separator();
    if ui.button("이름 변경").clicked() {
        actions.push(Action::Rename(path.to_path_buf()));
        ui.close();
    }
    if ui.button("삭제").clicked() {
        actions.push(Action::Delete(selection.clone(), false));
        ui.close();
    }
    if ui.button("영구 삭제").clicked() {
        actions.push(Action::Delete(selection.clone(), true));
        ui.close();
    }
    ui.separator();
    if ui.button("경로 복사").clicked() {
        actions.push(Action::CopyPath(selection.clone()));
        ui.close();
    }
    if ui.button("속성").clicked() {
        actions.push(Action::Properties(selection.clone()));
        ui.close();
    }
    ui.separator();
    ui.menu_button(format!("보기: {}", view_mode.label()), |ui| {
        for mode in ViewMode::all() {
            if ui
                .selectable_label(view_mode == mode, mode.label())
                .clicked()
            {
                actions.push(Action::SetView(mode));
                ui.close();
            }
        }
    });
    sort_menu(ui, actions, sort, sort_desc);
    let mut hidden = show_hidden;
    if ui.checkbox(&mut hidden, "숨김 항목 표시").changed() {
        actions.push(Action::SetHidden(hidden));
    }
    let mut extensions = show_extensions;
    if ui.checkbox(&mut extensions, "파일 확장명 표시").changed() {
        actions.push(Action::SetShowExtensions(extensions));
    }
    ui.separator();
    if ui.button("추가 옵션...").clicked() {
        actions.push(Action::ShellMenu {
            paths: selection,
            background: None,
        });
        ui.close();
    }
}

fn sort_menu(ui: &mut Ui, actions: &mut Vec<Action>, sort: SortKey, sort_desc: bool) {
    ui.menu_button(format!("정렬: {}", sort_label(sort)), |ui| {
        for (key, label) in [
            (SortKey::Name, "이름"),
            (SortKey::Size, "크기"),
            (SortKey::Modified, "수정일"),
            (SortKey::Created, "만든 날짜"),
            (SortKey::Accessed, "액세스한 날짜"),
            (SortKey::Type, "유형"),
        ] {
            if ui.selectable_label(sort == key, label).clicked() {
                actions.push(Action::SetSort(key));
                ui.close();
            }
        }
        ui.separator();
        let mut desc = sort_desc;
        if ui.checkbox(&mut desc, "내림차순").changed() {
            actions.push(Action::SetSortDesc(desc));
        }
    });
}

fn view_options(
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    view_mode: ViewMode,
    sort: SortKey,
    sort_desc: bool,
    show_hidden: bool,
    show_extensions: bool,
) {
    ui.set_min_width(220.0);
    ui.label(RichText::new("보기 옵션").strong());
    for mode in ViewMode::all() {
        if ui
            .selectable_label(view_mode == mode, mode.label())
            .clicked()
        {
            actions.push(Action::SetView(mode));
        }
    }
    ui.separator();
    let mut hidden = show_hidden;
    if ui.checkbox(&mut hidden, "숨김 항목 표시").changed() {
        actions.push(Action::SetHidden(hidden));
    }
    let mut extensions = show_extensions;
    if ui.checkbox(&mut extensions, "파일 확장명 표시").changed() {
        actions.push(Action::SetShowExtensions(extensions));
    }
    ui.separator();
    sort_menu(ui, actions, sort, sort_desc);
    ui.separator();
    if ui.button("새로 고침").clicked() {
        actions.push(Action::Refresh);
        ui.close();
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Back,
    Forward,
    Up,
    Refresh,
    NewFolder,
    Rename,
    Delete,
    Preview,
    LayoutOne,
    LayoutTwo,
    LayoutFour,
    ViewOptions,
    Sun,
    Moon,
}

fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, selected: bool) -> Response {
    let size = vec2(28.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.style().interact(&response);
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else {
        visuals.bg_fill
    };
    let stroke = if selected {
        ui.visuals().selection.stroke
    } else {
        visuals.bg_stroke
    };
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        visuals.fg_stroke.color
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, fill);
    painter.rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);
    draw_icon(painter, icon, rect.shrink(6.0), color, fill);
    response.on_hover_text(tooltip)
}

fn icon_button_enabled(ui: &mut Ui, enabled: bool, icon: Icon, tooltip: &str) -> Response {
    if enabled {
        return icon_button(ui, icon, tooltip, false);
    }
    let size = vec2(28.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let color = ui.visuals().weak_text_color().gamma_multiply(0.45);
    draw_icon(
        ui.painter(),
        icon,
        rect.shrink(6.0),
        color,
        Color32::TRANSPARENT,
    );
    response.on_hover_text(tooltip)
}

fn draw_icon(painter: &Painter, icon: Icon, rect: Rect, color: Color32, bg: Color32) {
    let stroke = Stroke::new(1.5, color);
    let center = rect.center();
    match icon {
        Icon::Back => {
            painter.line_segment(
                [
                    pos2(rect.right() - 1.0, center.y),
                    pos2(rect.left() + 1.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.left() + 1.0, center.y),
                    pos2(rect.left() + 5.0, center.y - 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.left() + 1.0, center.y),
                    pos2(rect.left() + 5.0, center.y + 4.0),
                ],
                stroke,
            );
        }
        Icon::Forward => {
            painter.line_segment(
                [
                    pos2(rect.left() + 1.0, center.y),
                    pos2(rect.right() - 1.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.right() - 1.0, center.y),
                    pos2(rect.right() - 5.0, center.y - 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.right() - 1.0, center.y),
                    pos2(rect.right() - 5.0, center.y + 4.0),
                ],
                stroke,
            );
        }
        Icon::Up => {
            painter.line_segment(
                [
                    pos2(center.x, rect.bottom() - 1.0),
                    pos2(center.x, rect.top() + 1.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x, rect.top() + 1.0),
                    pos2(center.x - 4.0, rect.top() + 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x, rect.top() + 1.0),
                    pos2(center.x + 4.0, rect.top() + 5.0),
                ],
                stroke,
            );
        }
        Icon::Refresh => {
            let radius = rect.width() * 0.34;
            let mut points = Vec::new();
            for step in 0..=18 {
                let angle = -0.65 + step as f32 / 18.0 * std::f32::consts::TAU * 0.82;
                points.push(center + vec2(angle.cos(), angle.sin()) * radius);
            }
            painter.add(egui::Shape::line(points, stroke));
            let tip = center + vec2(0.65f32.cos(), 0.65f32.sin()) * radius;
            painter.line_segment([tip, tip + vec2(-4.5, -0.5)], stroke);
            painter.line_segment([tip, tip + vec2(-1.0, 4.5)], stroke);
        }
        Icon::NewFolder => {
            let tab = Rect::from_min_max(
                pos2(rect.left() + 1.0, rect.top() + 1.0),
                pos2(rect.left() + rect.width() * 0.58, rect.top() + 5.0),
            );
            painter.rect_filled(tab, 1.0, color);
            let body = Rect::from_min_max(
                pos2(rect.left() + 1.0, rect.top() + 4.0),
                pos2(rect.right() - 1.0, rect.bottom() - 1.0),
            );
            painter.rect_stroke(body, 2.0, stroke, StrokeKind::Inside);
            let plus = pos2(rect.right() - 3.0, rect.bottom() - 3.0);
            painter.line_segment([plus - vec2(3.5, 0.0), plus + vec2(3.5, 0.0)], stroke);
            painter.line_segment([plus - vec2(0.0, 3.5), plus + vec2(0.0, 3.5)], stroke);
        }
        Icon::Rename => {
            let tip = pos2(rect.left() + 1.5, rect.bottom() - 1.5);
            let end = pos2(rect.right() - 2.0, rect.top() + 2.0);
            painter.line_segment([tip, end], Stroke::new(2.4, color));
            painter.add(egui::Shape::convex_polygon(
                vec![tip, tip + vec2(4.0, -1.0), tip + vec2(1.0, -4.0)],
                color,
                Stroke::NONE,
            ));
            painter.line_segment(
                [end + vec2(-2.5, 2.5), end + vec2(0.5, -0.5)],
                Stroke::new(1.5, color),
            );
        }
        Icon::Delete => {
            painter.line_segment(
                [
                    pos2(rect.left() + 2.0, rect.top() + 4.0),
                    pos2(rect.right() - 2.0, rect.top() + 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.left() + 5.0, rect.top() + 4.0),
                    pos2(rect.left() + 6.0, rect.top() + 1.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.left() + 6.0, rect.top() + 1.0),
                    pos2(rect.right() - 6.0, rect.top() + 1.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.right() - 6.0, rect.top() + 1.0),
                    pos2(rect.right() - 5.0, rect.top() + 4.0),
                ],
                stroke,
            );
            let body = Rect::from_min_max(
                pos2(rect.left() + 3.5, rect.top() + 5.5),
                pos2(rect.right() - 3.5, rect.bottom() - 1.0),
            );
            painter.rect_stroke(body, 1.0, stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    pos2(body.left() + 3.0, body.top() + 2.0),
                    pos2(body.left() + 3.0, body.bottom() - 2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(body.right() - 3.0, body.top() + 2.0),
                    pos2(body.right() - 3.0, body.bottom() - 2.0),
                ],
                stroke,
            );
        }
        Icon::Preview => {
            let left = pos2(rect.left() + 0.5, center.y);
            let right = pos2(rect.right() - 0.5, center.y);
            let top = pos2(center.x, rect.top() + 2.0);
            let bottom = pos2(center.x, rect.bottom() - 2.0);
            let mut points = Vec::new();
            for step in 0..=12 {
                points.push(quadratic_bezier(left, top, right, step as f32 / 12.0));
            }
            for step in 0..=12 {
                points.push(quadratic_bezier(right, bottom, left, step as f32 / 12.0));
            }
            painter.add(egui::Shape::line(points, stroke));
            painter.circle_filled(center, rect.width() * 0.13, color);
        }
        Icon::LayoutOne => {
            painter.rect_stroke(rect.shrink(1.0), 2.0, stroke, StrokeKind::Inside);
        }
        Icon::LayoutTwo => {
            let outer = rect.shrink(1.0);
            painter.rect_stroke(outer, 2.0, stroke, StrokeKind::Inside);
            painter.line_segment(
                [pos2(center.x, outer.top()), pos2(center.x, outer.bottom())],
                stroke,
            );
        }
        Icon::LayoutFour => {
            let outer = rect.shrink(1.0);
            painter.rect_stroke(outer, 2.0, stroke, StrokeKind::Inside);
            painter.line_segment(
                [pos2(center.x, outer.top()), pos2(center.x, outer.bottom())],
                stroke,
            );
            painter.line_segment(
                [pos2(outer.left(), center.y), pos2(outer.right(), center.y)],
                stroke,
            );
        }
        Icon::ViewOptions => {
            for (offset, knob) in [(-4.5, 0.32), (0.0, 0.66), (4.5, 0.42)] {
                let y = center.y + offset;
                painter.line_segment(
                    [pos2(rect.left() + 1.0, y), pos2(rect.right() - 1.0, y)],
                    Stroke::new(1.2, color),
                );
                painter.circle_filled(pos2(rect.left() + 1.0 + rect.width() * knob, y), 2.0, color);
            }
        }
        Icon::Sun => {
            painter.circle_stroke(center, 3.3, stroke);
            for step in 0..8 {
                let angle = step as f32 * std::f32::consts::TAU / 8.0;
                let direction = vec2(angle.cos(), angle.sin());
                painter.line_segment(
                    [center + direction * 5.2, center + direction * 7.0],
                    Stroke::new(1.3, color),
                );
            }
        }
        Icon::Moon => {
            painter.circle_filled(center + vec2(-0.5, 0.0), 5.2, color);
            painter.circle_filled(center + vec2(2.0, -1.6), 4.7, bg);
        }
    }
}

fn quadratic_bezier(start: egui::Pos2, control: egui::Pos2, end: egui::Pos2, t: f32) -> egui::Pos2 {
    let one_minus = 1.0 - t;
    egui::Pos2::new(
        one_minus * one_minus * start.x + 2.0 * one_minus * t * control.x + t * t * end.x,
        one_minus * one_minus * start.y + 2.0 * one_minus * t * control.y + t * t * end.y,
    )
}

struct Palette {
    pane_active: Color32,
    pane_inactive: Color32,
    pane_border: Color32,
    accent: Color32,
    path_bg: Color32,
    header_bg: Color32,
    header_text: Color32,
    row_selected: Color32,
    row_hover: Color32,
    text: Color32,
    text_weak: Color32,
    text_hidden: Color32,
    badge_text: Color32,
    danger: Color32,
    splitter: Color32,
    splitter_hover: Color32,
}

impl Palette {
    fn from_visuals(visuals: &egui::Visuals) -> Self {
        if visuals.dark_mode {
            Self {
                pane_active: Color32::from_rgb(29, 31, 35),
                pane_inactive: Color32::from_rgb(23, 24, 27),
                pane_border: Color32::from_rgb(49, 51, 56),
                accent: Color32::from_rgb(80, 140, 230),
                path_bg: Color32::from_rgb(34, 36, 41),
                header_bg: Color32::from_rgb(34, 36, 41),
                header_text: Color32::from_gray(145),
                row_selected: Color32::from_rgb(43, 72, 111),
                row_hover: Color32::from_rgb(37, 39, 44),
                text: Color32::from_gray(222),
                text_weak: Color32::from_gray(150),
                text_hidden: Color32::from_gray(140),
                badge_text: Color32::from_gray(245),
                danger: Color32::from_rgb(240, 110, 110),
                splitter: Color32::from_gray(43),
                splitter_hover: Color32::from_gray(120),
            }
        } else {
            Self {
                pane_active: Color32::WHITE,
                pane_inactive: Color32::from_rgb(246, 247, 249),
                pane_border: Color32::from_rgb(210, 214, 220),
                accent: Color32::from_rgb(30, 115, 210),
                path_bg: Color32::from_rgb(243, 246, 250),
                header_bg: Color32::from_rgb(237, 241, 246),
                header_text: Color32::from_rgb(95, 100, 110),
                row_selected: Color32::from_rgb(211, 229, 252),
                row_hover: Color32::from_rgb(240, 243, 247),
                text: Color32::from_rgb(28, 31, 36),
                text_weak: Color32::from_rgb(100, 105, 112),
                text_hidden: Color32::from_rgb(145, 150, 158),
                badge_text: Color32::from_rgb(25, 25, 25),
                danger: Color32::from_rgb(190, 40, 40),
                splitter: Color32::from_rgb(208, 212, 218),
                splitter_hover: Color32::from_rgb(130, 136, 146),
            }
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_visuals_of(egui::Theme::Light, light_visuals());
    ctx.set_visuals_of(egui::Theme::Dark, dark_visuals());
    ctx.set_theme(match theme {
        Theme::Light => egui::Theme::Light,
        Theme::Dark => egui::Theme::Dark,
    });
}

fn light_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = Color32::from_rgb(247, 248, 250);
    visuals.window_fill = Color32::WHITE;
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.faint_bg_color = Color32::from_rgb(241, 243, 247);
    visuals.code_bg_color = Color32::from_rgb(238, 241, 245);
    visuals.selection.bg_fill = Color32::from_rgb(211, 229, 252);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0, 70, 140));
    visuals.hyperlink_color = Color32::from_rgb(0, 92, 197);
    visuals.error_fg_color = Color32::from_rgb(190, 40, 40);
    visuals.warn_fg_color = Color32::from_rgb(160, 100, 0);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(242, 244, 248);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(236, 239, 244);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(231, 236, 244);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(224, 231, 241);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(180, 188, 200));
    visuals.widgets.active.bg_fill = Color32::from_rgb(211, 229, 252);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0, 92, 197));
    visuals
}

fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(22, 23, 26);
    visuals.window_fill = Color32::from_rgb(29, 31, 35);
    visuals.extreme_bg_color = Color32::from_rgb(18, 19, 22);
    visuals.faint_bg_color = Color32::from_additive_luminance(5);
    visuals.code_bg_color = Color32::from_rgb(40, 42, 47);
    visuals.selection.bg_fill = Color32::from_rgb(43, 72, 111);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(205, 225, 255));
    visuals
}

fn draw_header(
    ui: &mut Ui,
    palette: &Palette,
    columns: &mut ColumnWidths,
    sort: SortKey,
    sort_desc: bool,
) -> (Option<SortKey>, bool) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, palette.header_bg);
    let font = FontId::proportional(11.0);
    let color = palette.header_text;
    let (name_right, size_right, date_right, type_left) = columns_for(rect, columns);
    let arrow = |key: SortKey| {
        if sort == key {
            if sort_desc { " ▼" } else { " ▲" }
        } else {
            ""
        }
    };
    painter.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("이름{}", arrow(SortKey::Name)),
        font.clone(),
        color,
    );
    {
        painter.text(
            pos2(size_right, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("크기{}", arrow(SortKey::Size)),
            font.clone(),
            color,
        );
        painter.text(
            pos2(date_right, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("수정일{}", arrow(SortKey::Modified)),
            font.clone(),
            color,
        );
        painter.text(
            pos2(type_left, rect.center().y),
            Align2::LEFT_CENTER,
            format!("유형{}", arrow(SortKey::Type)),
            font,
            color,
        );
    }
    let _ = name_right;
    let mut resized = false;
    let mut near_handle = false;
    for (index, boundary) in [name_right, size_right, date_right].into_iter().enumerate() {
        let handle =
            Rect::from_center_size(pos2(boundary, rect.center().y), vec2(7.0, HEADER_HEIGHT));
        let handle_response = ui.interact(
            handle,
            ui.make_persistent_id(("column-resize", index)),
            Sense::drag(),
        );
        if handle_response.hovered() || handle_response.dragged() {
            near_handle = true;
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if handle_response.dragged()
            && let Some(position) = handle_response.interact_pointer_pos()
        {
            let ratio = ((position.x - rect.left()) / rect.width()).clamp(0.1, 0.9);
            let mut next = *columns;
            match index {
                0 => next.name = ratio,
                1 => next.size = (ratio - next.name).max(0.05),
                2 => next.modified = (ratio - next.name - next.size).max(0.05),
                _ => {}
            }
            *columns = next.normalized();
            resized = true;
        }
    }

    if !near_handle
        && response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        if pos.x >= type_left {
            return (Some(SortKey::Type), resized);
        }
        if pos.x >= date_right {
            return (Some(SortKey::Modified), resized);
        }
        if pos.x >= size_right {
            return (Some(SortKey::Size), resized);
        }
        return (Some(SortKey::Name), resized);
    }
    (None, resized)
}

fn draw_row(
    ui: &mut Ui,
    entry: &Entry,
    selected: bool,
    palette: &Palette,
    columns: &ColumnWidths,
    show_extensions: bool,
    drop_hover: bool,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    let painter = ui.painter();
    let fill = if drop_hover {
        palette.accent.gamma_multiply(0.32)
    } else if selected {
        palette.row_selected
    } else if response.hovered() {
        palette.row_hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, fill);
    }
    if drop_hover {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }

    let badge = Rect::from_min_size(
        pos2(rect.left() + 5.0, rect.center().y - 8.0),
        vec2(16.0, 16.0),
    );
    draw_badge(painter, entry, badge, palette, 8.0);

    let (name_right, size_right, date_right, type_left) = columns_for(rect, columns);
    let text_color = if entry.hidden {
        palette.text_hidden
    } else {
        palette.text
    };
    let name_rect = Rect::from_min_max(
        pos2(rect.left() + 26.0, rect.top()),
        pos2(name_right, rect.bottom()),
    );
    painter.with_clip_rect(name_rect.intersect(rect)).text(
        pos2(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        entry.display_name(show_extensions),
        FontId::proportional(12.5),
        text_color,
    );

    let secondary = palette.text_weak;
    painter.text(
        pos2(size_right, rect.center().y),
        Align2::RIGHT_CENTER,
        if entry.is_dir {
            String::new()
        } else {
            format_size(entry.size)
        },
        FontId::proportional(11.0),
        secondary,
    );
    painter.text(
        pos2(date_right, rect.center().y),
        Align2::RIGHT_CENTER,
        format_time(entry.modified),
        FontId::proportional(11.0),
        secondary,
    );
    painter
        .with_clip_rect(
            Rect::from_min_max(
                pos2(type_left, rect.top()),
                pos2(rect.right() - 4.0, rect.bottom()),
            )
            .intersect(rect),
        )
        .text(
            pos2(type_left, rect.center().y),
            Align2::LEFT_CENTER,
            entry.type_label(),
            FontId::proportional(11.0),
            secondary,
        );

    response
}

struct GridCellState {
    selected: bool,
    tile: bool,
    drop_hover: bool,
}

fn draw_grid_cell(
    ui: &mut Ui,
    entry: &Entry,
    palette: &Palette,
    show_extensions: bool,
    state: GridCellState,
    thumbnails: &mut ThumbnailStore,
) -> egui::Response {
    let size = if state.tile { TILE_CELL } else { ICON_CELL };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let thumbnail = if ui.is_rect_visible(rect.expand(160.0)) {
        thumbnails.texture_for(&entry.path)
    } else {
        None
    };
    let painter = ui.painter();
    let fill = if state.drop_hover {
        palette.accent.gamma_multiply(0.32)
    } else if state.selected {
        palette.row_selected
    } else if response.hovered() {
        palette.row_hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 4.0, fill);
    }
    if state.drop_hover {
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }
    let text_color = if entry.hidden {
        palette.text_hidden
    } else {
        palette.text
    };
    if state.tile {
        let badge = Rect::from_min_size(
            pos2(rect.left() + 8.0, rect.center().y - 14.0),
            vec2(28.0, 28.0),
        );
        if let Some(texture) = &thumbnail {
            painter.image(
                texture.id(),
                fit_rect(badge, texture.size()),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            draw_badge(painter, entry, badge, palette, 10.0);
        }
        let name_rect = Rect::from_min_max(
            pos2(rect.left() + 42.0, rect.top() + 8.0),
            pos2(rect.right() - 6.0, rect.bottom() - 6.0),
        );
        painter.with_clip_rect(name_rect).text(
            pos2(name_rect.left(), name_rect.top() + 3.0),
            Align2::LEFT_TOP,
            entry.display_name(show_extensions),
            FontId::proportional(12.0),
            text_color,
        );
        painter.with_clip_rect(name_rect).text(
            pos2(name_rect.left(), name_rect.top() + 22.0),
            Align2::LEFT_TOP,
            entry.type_label(),
            FontId::proportional(10.5),
            palette.text_weak,
        );
    } else {
        let badge =
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 30.0), vec2(44.0, 44.0));
        if let Some(texture) = &thumbnail {
            painter.image(
                texture.id(),
                fit_rect(badge, texture.size()),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            draw_badge(painter, entry, badge, palette, 12.0);
        }
        let name_rect = Rect::from_min_max(
            pos2(rect.left() + 4.0, badge.bottom() + 4.0),
            pos2(rect.right() - 4.0, rect.bottom() - 2.0),
        );
        painter.with_clip_rect(name_rect).text(
            pos2(rect.center().x, name_rect.top() + 2.0),
            Align2::CENTER_TOP,
            entry.display_name(show_extensions),
            FontId::proportional(11.0),
            text_color,
        );
    }
    response
}

fn fit_rect(rect: Rect, texture_size: [usize; 2]) -> Rect {
    let width = texture_size[0].max(1) as f32;
    let height = texture_size[1].max(1) as f32;
    let scale = (rect.width() / width).min(rect.height() / height);
    Rect::from_center_size(rect.center(), vec2(width * scale, height * scale))
}

fn draw_badge(painter: &Painter, entry: &Entry, rect: Rect, palette: &Palette, font_size: f32) {
    painter.rect_filled(rect, 4.0, badge_color(entry));
    let label = if entry.is_dir {
        "D".to_string()
    } else {
        let ext = entry.extension.to_uppercase();
        if ext.is_empty() {
            "·".to_string()
        } else {
            ext.chars().take(3).collect()
        }
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(font_size),
        palette.badge_text,
    );
}

fn columns_for(rect: Rect, columns: &ColumnWidths) -> (f32, f32, f32, f32) {
    let width = rect.width();
    let name_right = rect.left() + width * columns.name;
    let size_right = name_right + width * columns.size;
    let date_right = size_right + width * columns.modified;
    let type_left = date_right;
    (name_right, size_right, date_right, type_left)
}

fn badge_color(entry: &Entry) -> Color32 {
    if entry.is_dir {
        return Color32::from_rgb(196, 154, 62);
    }
    match entry.extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "avif" => {
            Color32::from_rgb(88, 140, 96)
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" => Color32::from_rgb(145, 96, 165),
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" => Color32::from_rgb(165, 88, 96),
        "pdf" => Color32::from_rgb(180, 72, 72),
        "zip" | "7z" | "rar" | "tar" | "gz" => Color32::from_rgb(120, 110, 90),
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "cs" | "go" | "java" | "sh" => {
            Color32::from_rgb(72, 110, 160)
        }
        "md" | "markdown" | "txt" | "log" => Color32::from_rgb(100, 110, 125),
        _ => Color32::from_rgb(86, 90, 98),
    }
}

fn format_time(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return String::new();
    };
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M").to_string()
}

fn sort_label(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Name => "이름",
        SortKey::Size => "크기",
        SortKey::Modified => "수정일",
        SortKey::Created => "만든 날짜",
        SortKey::Accessed => "액세스한 날짜",
        SortKey::Type => "유형",
    }
}

fn tab_count(pane: &Pane) -> usize {
    pane.tabs.len()
}

fn breadcrumb_segments(path: &Path) -> Vec<(String, PathBuf)> {
    let mut segments: Vec<(String, PathBuf)> = path
        .ancestors()
        .map(|ancestor| {
            let label = ancestor
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| ancestor.display().to_string());
            (label, ancestor.to_path_buf())
        })
        .collect();
    segments.reverse();
    segments
}

fn expand_home(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        if let Some(home) = home {
            let rest = text.trim_start_matches('~').trim_start_matches(['/', '\\']);
            return if rest.is_empty() {
                home
            } else {
                home.join(rest)
            };
        }
    }
    path
}

fn unique_name(parent: &Path, base: &str) -> String {
    if !parent.join(base).exists() {
        return base.to_string();
    }
    for index in 2..10_000 {
        let candidate = format!("{base} ({index})");
        if !parent.join(&candidate).exists() {
            return candidate;
        }
    }
    base.to_string()
}

fn is_valid_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return false;
    }
    if trimmed.chars().any(|ch| {
        ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return false;
    }
    #[cfg(windows)]
    {
        let stem = trimmed
            .split('.')
            .next()
            .unwrap_or(trimmed)
            .to_ascii_uppercase();
        const RESERVED: &[&str] = &[
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        if RESERVED.contains(&stem.as_str()) {
            return false;
        }
        if trimmed.ends_with('.') || trimmed.ends_with(' ') {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn attach_shell(cc: &eframe::CreationContext<'_>) -> Option<String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Err(error) = winfinder_shell::init() {
        return Some(format!("셸 초기화 실패: {error}"));
    }
    if let Ok(handle) = cc.window_handle()
        && let RawWindowHandle::Win32(win32) = handle.as_raw()
    {
        winfinder_shell::attach_window(win32.hwnd.get());
        if let Err(error) = winfinder_shell::register_drop_target() {
            return Some(format!("끌어다 놓기 초기화 실패: {error}"));
        }
        None
    } else {
        Some("창 핸들을 찾지 못해 셸 기능을 사용할 수 없습니다".to_string())
    }
}

#[cfg(not(windows))]
fn attach_shell(_cc: &eframe::CreationContext<'_>) -> Option<String> {
    let _ = winfinder_shell::init();
    None
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "pretendard".to_string(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/Pretendard-Regular.otf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "pretendard".to_string());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("pretendard".to_string());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_selection_scrolls_only_at_view_edges() {
        let row_pitch = ROW_HEIGHT + 4.0;
        let viewport = row_pitch * 10.0;

        assert_eq!(scroll_to_show_row(4, 0.0, viewport, row_pitch), 0.0);
        assert_eq!(scroll_to_show_row(9, 0.0, viewport, row_pitch), 0.0);
        assert_eq!(scroll_to_show_row(10, 0.0, viewport, row_pitch), row_pitch);
        assert_eq!(scroll_to_show_row(9, 5.0, viewport, row_pitch), 5.0);
        assert_eq!(
            scroll_to_show_row(20, 5.0, viewport, row_pitch),
            row_pitch * 11.0
        );
        assert_eq!(scroll_to_show_row(0, 5.0, viewport, row_pitch), 0.0);
    }

    #[test]
    fn split_axis_clamps_to_minimum_pane_size() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
        let (columns, handle) = split_axis(rect, 2, PANE_GAP, 0.0, MIN_PANE_WIDTH, true);
        assert_eq!(columns[0].width(), MIN_PANE_WIDTH);
        assert_eq!(handle.unwrap().width(), PANE_GAP);

        let (rows, _) = split_axis(rect, 2, PANE_GAP, 1.0, MIN_PANE_HEIGHT, false);
        assert_eq!(rows[1].height(), MIN_PANE_HEIGHT);
    }
}
