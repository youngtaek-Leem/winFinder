use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Selection {
    paths: BTreeSet<PathBuf>,
    anchor: Option<PathBuf>,
    focus: Option<PathBuf>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.paths.clear();
        self.anchor = None;
        self.focus = None;
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths.iter()
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.iter().cloned().collect()
    }

    pub fn focus(&self) -> Option<&Path> {
        self.focus
            .as_deref()
            .or_else(|| self.paths.iter().next().map(PathBuf::as_path))
    }

    pub fn primary(&self) -> Option<PathBuf> {
        self.focus().map(Path::to_path_buf)
    }

    pub fn anchor(&self) -> Option<&Path> {
        self.anchor.as_deref()
    }

    pub fn select_only(&mut self, path: PathBuf) {
        self.paths.clear();
        self.paths.insert(path.clone());
        self.focus = Some(path.clone());
        self.anchor = Some(path);
    }

    pub fn replace(&mut self, paths: impl IntoIterator<Item = PathBuf>, focus: Option<PathBuf>) {
        self.paths = paths.into_iter().collect();
        if let Some(focus) = focus.filter(|path| self.paths.contains(path)) {
            self.focus = Some(focus.clone());
            self.anchor = Some(focus);
        } else {
            self.focus = self.paths.iter().next().cloned();
            self.anchor = self.focus.clone();
        }
    }

    pub fn focus_only(&mut self, path: PathBuf) {
        self.focus = Some(path);
    }

    pub fn toggle(&mut self, path: PathBuf) {
        if self.paths.remove(&path) {
            if self.focus.as_ref() == Some(&path) {
                self.focus = self.paths.iter().next().cloned();
            }
            if self.anchor.as_ref() == Some(&path) {
                self.anchor = Some(path.clone());
            }
        } else {
            self.paths.insert(path.clone());
            self.focus = Some(path.clone());
            self.anchor = Some(path);
        }
    }

    pub fn select_range(&mut self, ordered: &[PathBuf], target: usize, additive: bool) {
        if ordered.is_empty() {
            self.clear();
            return;
        }
        let target = target.min(ordered.len() - 1);
        let anchor = self
            .anchor
            .as_ref()
            .and_then(|anchor| ordered.iter().position(|path| path == anchor))
            .unwrap_or(target);
        let (start, end) = if anchor <= target {
            (anchor, target)
        } else {
            (target, anchor)
        };
        if !additive {
            self.paths.clear();
        }
        for path in &ordered[start..=end] {
            self.paths.insert(path.clone());
        }
        self.focus = Some(ordered[target].clone());
        if self.anchor.is_none() {
            self.anchor = Some(ordered[anchor].clone());
        }
    }

    pub fn select_all(&mut self, ordered: &[PathBuf]) {
        self.paths = ordered.iter().cloned().collect();
        self.focus = ordered.last().cloned().or_else(|| self.focus.clone());
        if self.anchor.is_none() {
            self.anchor = ordered.first().cloned();
        }
    }

    pub fn retain_existing(&mut self, existing: &BTreeSet<PathBuf>) {
        self.paths.retain(|path| existing.contains(path));
        if self
            .focus
            .as_ref()
            .is_some_and(|path| !existing.contains(path))
        {
            self.focus = self.paths.iter().next().cloned();
        }
        if self
            .anchor
            .as_ref()
            .is_some_and(|path| !existing.contains(path))
        {
            self.anchor = self.focus.clone();
        }
    }

    pub fn remove_missing(&mut self, existing: &BTreeSet<PathBuf>) {
        self.retain_existing(existing);
    }

    pub fn clear_focus(&mut self) {
        self.focus = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn range_selection_uses_anchor() {
        let ordered = paths(&["a", "b", "c", "d"]);
        let mut selection = Selection::default();
        selection.select_only(PathBuf::from("b"));
        selection.select_range(&ordered, 3, false);
        let selected: Vec<_> = selection.paths();
        assert_eq!(selected, ordered[1..=3].to_vec());
        assert_eq!(selection.primary(), Some(PathBuf::from("d")));
    }

    #[test]
    fn additive_range_keeps_existing() {
        let ordered = paths(&["a", "b", "c", "d"]);
        let mut selection = Selection::default();
        selection.select_only(PathBuf::from("a"));
        selection.select_range(&ordered, 2, true);
        assert!(selection.contains(Path::new("a")));
        assert!(selection.contains(Path::new("b")));
        assert!(selection.contains(Path::new("c")));
    }

    #[test]
    fn toggle_tracks_focus() {
        let mut selection = Selection::default();
        selection.toggle(PathBuf::from("a"));
        selection.toggle(PathBuf::from("b"));
        assert_eq!(selection.len(), 2);
        assert_eq!(selection.primary(), Some(PathBuf::from("b")));
        selection.toggle(PathBuf::from("b"));
        assert_eq!(selection.len(), 1);
        assert_eq!(selection.primary(), Some(PathBuf::from("a")));
    }
}
