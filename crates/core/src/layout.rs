use serde::{Deserialize, Serialize};

pub const MAX_PANES: usize = 4;
pub const MAX_TABS_PER_PANE: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutPreset {
    #[default]
    One,
    OneByTwo,
    TwoByTwo,
}

impl LayoutPreset {
    pub fn label(self) -> &'static str {
        match self {
            LayoutPreset::One => "1x1",
            LayoutPreset::OneByTwo => "1x2",
            LayoutPreset::TwoByTwo => "2x2",
        }
    }

    pub fn columns(self) -> usize {
        match self {
            LayoutPreset::One => 1,
            LayoutPreset::OneByTwo => 2,
            LayoutPreset::TwoByTwo => 2,
        }
    }

    pub fn rows(self) -> usize {
        match self {
            LayoutPreset::One => 1,
            LayoutPreset::OneByTwo => 1,
            LayoutPreset::TwoByTwo => 2,
        }
    }

    pub fn pane_count(self) -> usize {
        self.columns() * self.rows()
    }

    pub fn all() -> [LayoutPreset; 3] {
        [
            LayoutPreset::One,
            LayoutPreset::OneByTwo,
            LayoutPreset::TwoByTwo,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_count_never_exceeds_limit() {
        for preset in LayoutPreset::all() {
            assert!(preset.pane_count() <= MAX_PANES);
        }
    }
}
