use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Details,
    List,
    Icons,
    Tiles,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Details => "자세히",
            ViewMode::List => "목록",
            ViewMode::Icons => "아이콘",
            ViewMode::Tiles => "타일",
        }
    }

    pub fn all() -> [ViewMode; 4] {
        [
            ViewMode::Details,
            ViewMode::List,
            ViewMode::Icons,
            ViewMode::Tiles,
        ]
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ColumnWidths {
    pub name: f32,
    pub size: f32,
    pub modified: f32,
    pub kind: f32,
}

impl ColumnWidths {
    pub fn normalized(mut self) -> Self {
        let values = [self.name, self.size, self.modified, self.kind];
        let sum: f32 = values.iter().sum();
        if !sum.is_finite() || sum <= 0.0 {
            return Self::default();
        }
        self.name = (self.name / sum).clamp(0.20, 0.70);
        self.size = (self.size / sum).clamp(0.08, 0.30);
        self.modified = (self.modified / sum).clamp(0.10, 0.40);
        self.kind = (self.kind / sum).clamp(0.06, 0.30);
        let total = self.name + self.size + self.modified + self.kind;
        if total > 0.0 {
            self.name /= total;
            self.size /= total;
            self.modified /= total;
            self.kind /= total;
        }
        self
    }

    pub fn name_right(&self, width: f32) -> f32 {
        width * self.name
    }

    pub fn size_right(&self, width: f32) -> f32 {
        width * (self.name + self.size)
    }

    pub fn modified_right(&self, width: f32) -> f32 {
        width * (self.name + self.size + self.modified)
    }

    pub fn kind_left(&self, width: f32) -> f32 {
        self.modified_right(width)
    }
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            name: 0.50,
            size: 0.16,
            modified: 0.23,
            kind: 0.11,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_widths_normalize() {
        let columns = ColumnWidths {
            name: 2.0,
            size: 1.0,
            modified: 1.0,
            kind: 1.0,
        }
        .normalized();
        let total = columns.name + columns.size + columns.modified + columns.kind;
        assert!((total - 1.0).abs() < 0.001);
    }
}
