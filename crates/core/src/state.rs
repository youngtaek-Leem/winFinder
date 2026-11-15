use crate::entry::SortKey;
use crate::layout::{LayoutPreset, MAX_PANES, MAX_TABS_PER_PANE};
use crate::nav::History;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const STATE_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    #[serde(default)]
    pub window: WindowState,
    #[serde(default)]
    pub preset: LayoutPreset,
    #[serde(default = "default_split")]
    pub split_x: f32,
    #[serde(default = "default_split")]
    pub split_y: f32,
    #[serde(default)]
    pub active_pane: usize,
    #[serde(default)]
    pub panes: Vec<PaneState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub position: Option<[f32; 2]>,
    pub maximized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneState {
    #[serde(default)]
    pub tabs: Vec<TabState>,
    #[serde(default)]
    pub active_tab: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TabState {
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub sort: SortKey,
    #[serde(default)]
    pub sort_desc: bool,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub selected: Option<PathBuf>,
    #[serde(default)]
    pub scroll: f32,
}

fn default_split() -> f32 {
    0.5
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 800.0,
            position: None,
            maximized: false,
        }
    }
}

impl Default for TabState {
    fn default() -> Self {
        Self {
            history: History::new(default_dir()),
            sort: SortKey::Name,
            sort_desc: false,
            show_hidden: true,
            selected: None,
            scroll: 0.0,
        }
    }
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            tabs: vec![TabState::default()],
            active_tab: 0,
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            window: WindowState::default(),
            preset: LayoutPreset::OneByTwo,
            split_x: default_split(),
            split_y: default_split(),
            active_pane: 0,
            panes: (0..MAX_PANES).map(|_| PaneState::default()).collect(),
        }
    }
}

impl Session {
    pub fn normalize(&mut self) {
        let previous_version = self.version;
        self.split_x = self.split_x.clamp(0.1, 0.9);
        self.split_y = self.split_y.clamp(0.1, 0.9);

        if self.panes.is_empty() {
            self.panes = (0..MAX_PANES).map(|_| PaneState::default()).collect();
        }
        self.panes.truncate(MAX_PANES);
        while self.panes.len() < MAX_PANES {
            self.panes.push(PaneState::default());
        }

        for pane in &mut self.panes {
            if pane.tabs.is_empty() {
                pane.tabs.push(TabState::default());
            }
            pane.tabs.truncate(MAX_TABS_PER_PANE);
            if pane.active_tab >= pane.tabs.len() {
                pane.active_tab = pane.tabs.len() - 1;
            }
            for tab in &mut pane.tabs {
                tab.history.normalize();
            }
        }

        if self.active_pane >= self.panes.len() {
            self.active_pane = 0;
        }
        if self.window.width < 480.0 || self.window.height < 320.0 {
            self.window = WindowState::default();
        }
        if previous_version < 2 {
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    tab.show_hidden = true;
                }
            }
        }
        self.version = STATE_VERSION;
    }

    pub fn load() -> Option<Self> {
        let path = state_path()?;
        let bytes = std::fs::read(path).ok()?;
        let mut session: Session = serde_json::from_slice(&bytes).ok()?;
        session.normalize();
        Some(session)
    }

    pub fn save(&self) -> io::Result<()> {
        let path = state_path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "config directory not available")
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                std::fs::rename(&tmp, &path)
            }
        }
    }
}

pub fn state_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("winFinder").join("state.json"))
}

pub fn default_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .or_else(|| std::env::current_dir().ok())
        .or_else(|| crate::fs::roots().into_iter().next())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_enforces_limits() {
        let mut session = Session {
            panes: (0..10).map(|_| PaneState::default()).collect(),
            ..Session::default()
        };
        for pane in &mut session.panes {
            pane.tabs = (0..20).map(|_| TabState::default()).collect();
            pane.active_tab = 99;
        }
        session.normalize();
        assert_eq!(session.panes.len(), MAX_PANES);
        for pane in &session.panes {
            assert_eq!(pane.tabs.len(), MAX_TABS_PER_PANE);
            assert!(pane.active_tab < pane.tabs.len());
        }
    }

    #[test]
    fn round_trips_json() {
        let session = Session::default();
        let json = serde_json::to_vec(&session).unwrap();
        let decoded: Session = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded.panes.len(), MAX_PANES);
        assert_eq!(decoded.split_x, 0.5);
        assert_eq!(decoded.split_y, 0.5);
    }

    #[test]
    fn migration_shows_hidden_entries_and_clamps_splits() {
        let mut session = Session {
            version: 1,
            split_x: 0.0,
            split_y: 2.0,
            ..Session::default()
        };
        for pane in &mut session.panes {
            for tab in &mut pane.tabs {
                tab.show_hidden = false;
            }
        }

        session.normalize();

        assert_eq!(session.version, STATE_VERSION);
        assert_eq!(session.split_x, 0.1);
        assert_eq!(session.split_y, 0.9);
        assert!(
            session
                .panes
                .iter()
                .flat_map(|pane| &pane.tabs)
                .all(|tab| tab.show_hidden)
        );
    }
}
