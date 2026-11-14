use crate::audio::AudioPlayer;
use crate::video::VideoPlayer;
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Key, Layout,
    Modifiers, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
    UiBuilder, pos2, vec2,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use winfinder_core::{
    Entry, History, LayoutPreset, SortKey, TabState, list_dir, quick_access, roots, sort_entries,
};
use winfinder_preview::{Preview, format_size};

const ROW_HEIGHT: f32 = 24.0;
const HEADER_HEIGHT: f32 = 22.0;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const STATUS_TTL: Duration = Duration::from_secs(4);

pub struct WinFinderApp {
    panes: Vec<Pane>,
    preset: LayoutPreset,
    active_pane: usize,
    preview: PreviewState,
    cm_cache: egui_commonmark::CommonMarkCache,
    status: Option<(String, Instant)>,
    dirty: bool,
    last_save: Instant,
    window_state: winfinder_core::WindowState,
    dialog: Option<Dialog>,
}

struct Pane {
    tabs: Vec<Tab>,
    active: usize,
}

struct Tab {
    history: History,
    entries: Vec<Entry>,
    view: Vec<usize>,
    selected: Option<PathBuf>,
    sort: SortKey,
    sort_desc: bool,
    show_hidden: bool,
    query: String,
    scroll: f32,
    loaded: bool,
    error: Option<String>,
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
    Rename { path: PathBuf, name: String },
    NewFolder { parent: PathBuf, name: String },
    Delete { path: PathBuf },
}

enum Action {
    Select(PathBuf),
    Open(PathBuf),
    Navigate(PathBuf),
    Back,
    Forward,
    Up,
    NewTab,
    CloseTab(usize),
    SwitchTab(usize),
    SetSort(SortKey),
    SetSortDesc(bool),
    SetHidden(bool),
    SetQuery(String),
}

impl Tab {
    fn from_state(state: TabState) -> Self {
        Self {
            history: state.history,
            entries: Vec::new(),
            view: Vec::new(),
            selected: state.selected,
            sort: state.sort,
            sort_desc: state.sort_desc,
            show_hidden: state.show_hidden,
            query: String::new(),
            scroll: state.scroll,
            loaded: false,
            error: None,
        }
    }

    fn to_state(&self) -> TabState {
        TabState {
            history: self.history.clone(),
            sort: self.sort,
            sort_desc: self.sort_desc,
            show_hidden: self.show_hidden,
            selected: self.selected.clone(),
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

    fn load_current(&mut self) {
        let Some(path) = self.history.current().cloned() else {
            self.entries.clear();
            self.view.clear();
            self.error = Some("경로가 없습니다".to_string());
            self.loaded = true;
            return;
        };
        self.load_path(&path);
        self.loaded = true;
    }

    fn load_path(&mut self, path: &std::path::Path) -> bool {
        match list_dir(path) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
                self.apply_sort();
                true
            }
            Err(err) => {
                self.entries.clear();
                self.view.clear();
                self.error = Some(format!("{}: {err}", path.display()));
                false
            }
        }
    }

    fn goto(&mut self, path: PathBuf) -> bool {
        if self.load_path(&path) {
            self.history.push(path);
            self.selected = None;
            self.query.clear();
            self.scroll = 0.0;
            self.loaded = true;
            true
        } else {
            false
        }
    }

    fn apply_sort(&mut self) {
        sort_entries(&mut self.entries, self.sort, self.sort_desc);
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        let query = self.query.to_lowercase();
        self.view = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                (self.show_hidden || !entry.is_hidden())
                    && (query.is_empty() || entry.name.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect();
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
    pub fn new(cc: &eframe::CreationContext<'_>, mut session: winfinder_core::Session) -> Self {
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        session.normalize();
        let mut panes = Vec::new();
        for pane_state in session.panes {
            let mut tabs: Vec<Tab> = pane_state.tabs.into_iter().map(Tab::from_state).collect();
            if tabs.is_empty() {
                tabs.push(Tab::from_state(TabState::default()));
            }
            let active = pane_state.active_tab.min(tabs.len() - 1);
            panes.push(Pane { tabs, active });
        }
        for pane in &mut panes {
            pane.active_tab_mut().load_current();
        }

        Self {
            panes,
            preset: session.preset,
            active_pane: session.active_pane,
            preview: PreviewState::default(),
            cm_cache: egui_commonmark::CommonMarkCache::default(),
            status: None,
            dirty: false,
            last_save: Instant::now(),
            window_state: session.window,
            dialog: None,
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.panes
            .get(self.active_pane)?
            .active_tab()
            .selected
            .clone()
    }

    fn toggle_preview(&mut self, ctx: &egui::Context) {
        if self.preview.open {
            self.close_preview();
            return;
        }
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
        let ok = self
            .panes
            .get_mut(pane_idx)
            .map(|pane| pane.active_tab_mut().goto(path))
            .unwrap_or(false);
        if ok {
            self.dirty = true;
            self.close_preview();
            if !self.preview.open {
                ctx.request_repaint();
            }
        } else if let Some(pane) = self.panes.get(pane_idx)
            && let Some(err) = &pane.active_tab().error
        {
            self.set_status(err.clone());
        }
    }

    fn go_back(&mut self, pane_idx: usize) {
        let changed = if let Some(pane) = self.panes.get_mut(pane_idx) {
            let tab = pane.active_tab_mut();
            if let Some(path) = tab.history.back() {
                tab.loaded = true;
                tab.load_path(&path)
            } else {
                false
            }
        } else {
            false
        };
        if changed {
            self.dirty = true;
            self.close_preview();
        }
    }

    fn go_forward(&mut self, pane_idx: usize) {
        let changed = if let Some(pane) = self.panes.get_mut(pane_idx) {
            let tab = pane.active_tab_mut();
            if let Some(path) = tab.history.forward() {
                tab.loaded = true;
                tab.load_path(&path)
            } else {
                false
            }
        } else {
            false
        };
        if changed {
            self.dirty = true;
            self.close_preview();
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
        let current = pane
            .active_tab()
            .history
            .current()
            .cloned()
            .unwrap_or_else(winfinder_core::state::default_dir);
        let mut tab = Tab::from_state(TabState {
            history: History::new(current),
            ..TabState::default()
        });
        tab.load_current();
        pane.tabs.push(tab);
        pane.active = pane.tabs.len() - 1;
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
        pane.active_tab_mut().load_current();
        self.dirty = true;
        self.close_preview();
    }

    fn switch_tab(&mut self, pane_idx: usize, index: usize) {
        let Some(pane) = self.panes.get_mut(pane_idx) else {
            return;
        };
        if index >= pane.tabs.len() {
            return;
        }
        pane.active = index;
        pane.active_tab_mut().load_current();
        self.dirty = true;
        self.close_preview();
    }

    fn move_selection(&mut self, ctx: &egui::Context, delta: isize) {
        let changed = {
            let Some(pane) = self.panes.get_mut(self.active_pane) else {
                return;
            };
            let tab = pane.active_tab_mut();
            if tab.view.is_empty() {
                return;
            }
            let current = tab.selected.as_ref().and_then(|selected| {
                tab.view
                    .iter()
                    .position(|&index| &tab.entries[index].path == selected)
            });
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
            let entry = &tab.entries[tab.view[next]];
            tab.selected = Some(entry.path.clone());
            tab.scroll = (next as f32 * ROW_HEIGHT - 4.0 * ROW_HEIGHT).max(0.0);
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

    fn reload_active(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.active_pane) {
            pane.active_tab_mut().load_current();
        }
        self.dirty = true;
    }

    fn start_rename(&mut self) {
        let Some(path) = self.selected_path() else {
            self.set_status("이름을 변경할 항목을 선택하세요");
            return;
        };
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
        self.dialog = Some(Dialog::NewFolder {
            parent,
            name: "새 폴더".to_string(),
        });
    }

    fn start_delete(&mut self) {
        let Some(path) = self.selected_path() else {
            self.set_status("삭제할 항목을 선택하세요");
            return;
        };
        self.dialog = Some(Dialog::Delete { path });
    }

    fn apply_dialog(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Rename { path, name } => {
                let name = name.trim();
                if name.is_empty() {
                    self.set_status("이름을 입력하세요");
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
                match std::fs::rename(&path, &target) {
                    Ok(()) => {
                        self.reload_active();
                        if let Some(tab) = self.panes.get_mut(self.active_pane) {
                            tab.active_tab_mut().selected = Some(target);
                        }
                        self.set_status("이름을 변경했습니다");
                    }
                    Err(err) => self.set_status(format!("이름 변경 실패: {err}")),
                }
            }
            Dialog::NewFolder { parent, name } => {
                let name = name.trim();
                if name.is_empty() {
                    self.set_status("폴더 이름을 입력하세요");
                    return;
                }
                let target = parent.join(name);
                if target.exists() {
                    self.set_status(format!("이미 존재합니다: {}", target.display()));
                    return;
                }
                match std::fs::create_dir(&target) {
                    Ok(()) => {
                        self.reload_active();
                        if let Some(tab) = self.panes.get_mut(self.active_pane) {
                            tab.active_tab_mut().selected = Some(target);
                        }
                        self.set_status("새 폴더를 만들었습니다");
                    }
                    Err(err) => self.set_status(format!("폴더 생성 실패: {err}")),
                }
            }
            Dialog::Delete { path } => match trash::delete(&path) {
                Ok(()) => {
                    self.reload_active();
                    self.close_preview();
                    self.set_status("휴지통으로 이동했습니다");
                }
                Err(err) => self.set_status(format!("삭제 실패: {err}")),
            },
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
                    Dialog::Delete { path } => {
                        ui.label(format!(
                            "다음 항목을 휴지통으로 이동할까요?\n\n{}",
                            path.display()
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
        if ctx.egui_wants_keyboard_input() {
            if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
                self.close_preview();
            }
            return;
        }

        let mut space = false;
        let mut up = false;
        let mut down = false;
        let mut enter = false;
        let mut backspace = false;
        let mut escape = false;
        let mut back = false;
        let mut forward = false;
        let mut new_tab = false;
        let mut close_tab = false;
        let mut next_tab = false;
        let mut prev_tab = false;
        let mut rename = false;
        let mut delete = false;
        let mut new_folder = false;
        let mut pane_shortcut: Option<usize> = None;

        ctx.input_mut(|input| {
            space = input.consume_key(Modifiers::NONE, Key::Space);
            up = input.consume_key(Modifiers::NONE, Key::ArrowUp);
            down = input.consume_key(Modifiers::NONE, Key::ArrowDown);
            enter = input.consume_key(Modifiers::NONE, Key::Enter);
            backspace = input.consume_key(Modifiers::NONE, Key::Backspace);
            escape = input.consume_key(Modifiers::NONE, Key::Escape);
            back = input.consume_key(Modifiers::COMMAND, Key::ArrowLeft);
            forward = input.consume_key(Modifiers::COMMAND, Key::ArrowRight);
            new_tab = input.consume_key(Modifiers::COMMAND, Key::T);
            close_tab = input.consume_key(Modifiers::COMMAND, Key::W);
            next_tab = input.consume_key(Modifiers::COMMAND, Key::Tab);
            prev_tab = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab);
            rename = input.consume_key(Modifiers::NONE, Key::F2);
            delete = input.consume_key(Modifiers::NONE, Key::Delete);
            new_folder = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::N);
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
            self.move_selection(ctx, -1);
        }
        if down {
            self.move_selection(ctx, 1);
        }
        if enter {
            self.open_selected(ctx);
        }
        if backspace {
            self.go_up(ctx, self.active_pane);
        }
        if back {
            self.go_back(self.active_pane);
        }
        if forward {
            self.go_forward(self.active_pane);
        }
        if new_tab {
            self.new_tab(self.active_pane);
        }
        if new_folder {
            self.start_new_folder();
        }
        if rename {
            self.start_rename();
        }
        if delete {
            self.start_delete();
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
                    pane.active_tab_mut().selected = Some(path);
                }
                self.dirty = true;
                self.after_selection_changed(ctx);
            }
            Action::Open(path) => self.open_path(ctx, pane_idx, path),
            Action::Navigate(path) => self.navigate(ctx, pane_idx, path),
            Action::Back => self.go_back(pane_idx),
            Action::Forward => self.go_forward(pane_idx),
            Action::Up => self.go_up(ctx, pane_idx),
            Action::NewTab => self.new_tab(pane_idx),
            Action::CloseTab(index) => self.close_tab(pane_idx, index),
            Action::SwitchTab(index) => self.switch_tab(pane_idx, index),
            Action::SetSort(key) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    tab.sort = key;
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
            Action::SetHidden(show) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    tab.show_hidden = show;
                    tab.rebuild_view();
                }
                self.dirty = true;
            }
            Action::SetQuery(query) => {
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    let tab = pane.active_tab_mut();
                    tab.query = query;
                    tab.rebuild_view();
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("winFinder").strong().size(16.0));
            ui.separator();
            for preset in LayoutPreset::all() {
                if ui
                    .selectable_label(self.preset == preset, preset.label())
                    .clicked()
                {
                    self.preset = preset;
                    self.active_pane = self.active_pane.min(preset.pane_count() - 1);
                    self.dirty = true;
                }
            }
            ui.separator();
            if ui.button("미리보기 (Space)").clicked() {
                self.toggle_preview(ui.ctx());
            }
            ui.separator();
            if ui.button("새 폴더").clicked() {
                self.start_new_folder();
            }
            if ui.button("이름 변경").clicked() {
                self.start_rename();
            }
            if ui.button("삭제").clicked() {
                self.start_delete();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("↑↓ 이동 · Enter 열기 · Backspace 상위 · Ctrl+T/W 탭")
                        .color(Color32::from_gray(140))
                        .size(11.0),
                );
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
        ui.horizontal(|ui| {
            if let Some((message, at)) = &self.status {
                if at.elapsed() < STATUS_TTL {
                    ui.label(RichText::new(message).color(Color32::from_rgb(120, 190, 255)));
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
                ui.label(RichText::new(path).color(Color32::from_gray(150)));
                if let Some(error) = &tab.error {
                    ui.colored_label(Color32::from_rgb(240, 110, 110), error);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tab = self.panes[self.active_pane].active_tab();
                ui.label(
                    RichText::new(format!(
                        "{}개 항목 · 탭 {}/{}",
                        tab.view.len(),
                        self.panes[self.active_pane].active + 1,
                        tab_count(&self.panes[self.active_pane])
                    ))
                    .color(Color32::from_gray(130))
                    .size(11.0),
                );
            });
        });
    }

    fn grid(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let cols = self.preset.columns();
        let rows = self.preset.rows();
        let gap = 6.0;
        let cell_width = ((rect.width() - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
        let cell_height = ((rect.height() - gap * (rows as f32 - 1.0)) / rows as f32).max(1.0);

        for index in 0..self.preset.pane_count() {
            let col = index % cols;
            let row = index / cols;
            let min = pos2(
                rect.left() + col as f32 * (cell_width + gap),
                rect.top() + row as f32 * (cell_height + gap),
            );
            let cell = Rect::from_min_size(min, vec2(cell_width, cell_height));
            let active = index == self.active_pane;
            let fill = if active {
                Color32::from_rgb(29, 31, 35)
            } else {
                Color32::from_rgb(23, 24, 27)
            };
            ui.painter().rect_filled(cell, 6.0, fill);
            ui.painter().rect_stroke(
                cell,
                6.0,
                Stroke::new(
                    if active { 1.8 } else { 1.0 },
                    if active {
                        Color32::from_rgb(80, 140, 230)
                    } else {
                        Color32::from_rgb(49, 51, 56)
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

        if !self.panes[pane_index].active_tab().loaded {
            self.panes[pane_index].active_tab_mut().load_current();
            self.dirty = true;
        }

        let actions = pane_contents(ui, &mut self.panes[pane_index], pane_index);
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
            preset: self.preset,
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
        self.handle_shortcuts(&ctx);

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
    }
}

fn pane_contents(ui: &mut Ui, pane: &mut Pane, pane_index: usize) -> Vec<Action> {
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

    ui.horizontal(|ui| {
        if ui
            .add_enabled(can_back, egui::Button::new("←"))
            .on_hover_text("뒤로 (Ctrl/Cmd+←)")
            .clicked()
        {
            actions.push(Action::Back);
        }
        if ui
            .add_enabled(can_forward, egui::Button::new("→"))
            .on_hover_text("앞으로 (Ctrl/Cmd+→)")
            .clicked()
        {
            actions.push(Action::Forward);
        }
        if ui
            .button("↑")
            .on_hover_text("상위 폴더 (Backspace)")
            .clicked()
        {
            actions.push(Action::Up);
        }

        egui::ScrollArea::horizontal()
            .id_salt(("breadcrumb", pane_index, active_index))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut accumulated = PathBuf::new();
                    let components: Vec<_> = current_path.components().collect();
                    for (index, component) in components.iter().enumerate() {
                        accumulated.push(component.as_os_str());
                        if index > 0 {
                            ui.label(RichText::new("›").color(Color32::from_gray(110)));
                        }
                        let label = component.as_os_str().to_string_lossy();
                        if ui.small_button(label.as_ref()).clicked() {
                            actions.push(Action::Navigate(accumulated.clone()));
                        }
                    }
                });
            });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut query = tab.query.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text("필터")
                        .desired_width(100.0),
                )
                .changed()
            {
                actions.push(Action::SetQuery(query));
            }
            let mut show_hidden = tab.show_hidden;
            if ui.checkbox(&mut show_hidden, "숨김").changed() {
                actions.push(Action::SetHidden(show_hidden));
            }
            let mut sort = tab.sort;
            egui::ComboBox::from_id_salt(("sort", pane_index, active_index))
                .selected_text(sort_label(tab.sort))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sort, SortKey::Name, "이름");
                    ui.selectable_value(&mut sort, SortKey::Size, "크기");
                    ui.selectable_value(&mut sort, SortKey::Modified, "수정일");
                    ui.selectable_value(&mut sort, SortKey::Type, "유형");
                });
            if sort != tab.sort {
                actions.push(Action::SetSort(sort));
            }
            let mut desc = tab.sort_desc;
            if ui
                .selectable_label(desc, if desc { "↓" } else { "↑" })
                .on_hover_text("정렬 방향")
                .clicked()
            {
                desc = !desc;
                actions.push(Action::SetSortDesc(desc));
            }
            ui.label(
                RichText::new(truncate_path(&current_path, 44)).color(Color32::from_gray(130)),
            );
        });
    });

    if let Some(error) = &tab.error {
        ui.colored_label(Color32::from_rgb(240, 110, 110), error);
    }

    draw_header(ui);

    let selected = tab.selected.clone();
    let mut action = None;
    let view_len = tab.view.len();
    let output = egui::ScrollArea::vertical()
        .id_salt(("files", pane_index, active_index))
        .auto_shrink([false, false])
        .vertical_scroll_offset(tab.scroll)
        .show_rows(ui, ROW_HEIGHT, view_len, |ui, range| {
            for row in range {
                let entry = &tab.entries[tab.view[row]];
                let is_selected = selected.as_deref() == Some(entry.path.as_path());
                let response = draw_row(ui, entry, is_selected);
                if response.double_clicked() {
                    action = Some(Action::Open(entry.path.clone()));
                } else if response.clicked() {
                    action = Some(Action::Select(entry.path.clone()));
                }
            }
        });
    tab.scroll = output.state.clamped_offset().y;
    if let Some(action) = action {
        actions.push(action);
    }

    actions
}

fn draw_header(ui: &mut Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::from_rgb(34, 36, 41));
    let font = FontId::proportional(11.0);
    let color = Color32::from_gray(145);
    let (name_right, size_right, date_right, type_left) = columns(rect);
    painter.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        "이름",
        font.clone(),
        color,
    );
    painter.text(
        pos2(size_right, rect.center().y),
        Align2::RIGHT_CENTER,
        "크기",
        font.clone(),
        color,
    );
    painter.text(
        pos2(date_right, rect.center().y),
        Align2::RIGHT_CENTER,
        "수정일",
        font.clone(),
        color,
    );
    painter.text(
        pos2(type_left, rect.center().y),
        Align2::LEFT_CENTER,
        "유형",
        font,
        color,
    );
    let _ = name_right;
}

fn draw_row(ui: &mut Ui, entry: &Entry, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        Color32::from_rgb(43, 72, 111)
    } else if response.hovered() {
        Color32::from_rgb(37, 39, 44)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, fill);
    }

    let badge = Rect::from_min_size(
        pos2(rect.left() + 5.0, rect.center().y - 8.0),
        vec2(16.0, 16.0),
    );
    painter.rect_filled(badge, 3.0, badge_color(entry));
    let badge_text = if entry.is_dir {
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
        badge.center(),
        Align2::CENTER_CENTER,
        badge_text,
        FontId::proportional(8.0),
        Color32::from_gray(245),
    );

    let (name_right, size_right, date_right, type_left) = columns(rect);
    let text_color = if entry.hidden {
        Color32::from_gray(140)
    } else {
        Color32::from_gray(222)
    };
    let name_rect = Rect::from_min_max(
        pos2(rect.left() + 26.0, rect.top()),
        pos2(name_right, rect.bottom()),
    );
    painter.with_clip_rect(name_rect.intersect(rect)).text(
        pos2(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        &entry.name,
        FontId::proportional(12.5),
        text_color,
    );

    let secondary = Color32::from_gray(150);
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

fn columns(rect: Rect) -> (f32, f32, f32, f32) {
    (
        rect.left() + rect.width() * 0.50,
        rect.left() + rect.width() * 0.66,
        rect.left() + rect.width() * 0.89,
        rect.left() + rect.width() * 0.91,
    )
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
        SortKey::Type => "유형",
    }
}

fn tab_count(pane: &Pane) -> usize {
    pane.tabs.len()
}

fn truncate_path(path: &std::path::Path, max_chars: usize) -> String {
    let text = path.display().to_string();
    if text.chars().count() <= max_chars {
        return text;
    }
    let keep = max_chars.saturating_sub(1);
    let tail: String = text
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
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
