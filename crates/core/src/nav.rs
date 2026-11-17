use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    stack: Vec<PathBuf>,
    index: usize,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self {
            stack: vec![path],
            index: 0,
        }
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.stack.get(self.index)
    }

    pub fn push(&mut self, path: PathBuf) {
        if self.current() == Some(&path) {
            return;
        }
        self.stack.truncate(self.index + 1);
        self.stack.push(path);
        self.index = self.stack.len() - 1;
    }

    pub fn back_target(&self) -> Option<PathBuf> {
        if self.index > 0 {
            self.stack.get(self.index - 1).cloned()
        } else {
            None
        }
    }

    pub fn forward_target(&self) -> Option<PathBuf> {
        self.stack.get(self.index + 1).cloned()
    }

    pub fn back(&mut self) -> Option<PathBuf> {
        if self.index > 0 {
            self.index -= 1;
            return self.current().cloned();
        }
        None
    }

    pub fn forward(&mut self) -> Option<PathBuf> {
        if self.index + 1 < self.stack.len() {
            self.index += 1;
            return self.current().cloned();
        }
        None
    }

    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.stack.len()
    }

    pub fn parent(&self) -> Option<PathBuf> {
        self.current()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
    }

    pub fn normalize(&mut self) {
        if self.stack.is_empty() {
            self.stack.push(PathBuf::from("."));
        }
        if self.index >= self.stack.len() {
            self.index = self.stack.len() - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_forward_preserves_history() {
        let mut history = History::new(PathBuf::from("/a"));
        history.push(PathBuf::from("/b"));
        history.push(PathBuf::from("/c"));
        assert_eq!(history.back(), Some(PathBuf::from("/b")));
        assert_eq!(history.back(), Some(PathBuf::from("/a")));
        assert_eq!(history.forward(), Some(PathBuf::from("/b")));
        history.push(PathBuf::from("/d"));
        assert!(!history.can_forward());
    }
}
