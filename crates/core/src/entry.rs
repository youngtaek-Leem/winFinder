use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::SystemTime;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub attributes: u32,
    pub hidden: bool,
    pub extension: String,
}

impl Entry {
    pub fn is_hidden(&self) -> bool {
        self.hidden || self.attributes & FILE_ATTRIBUTE_HIDDEN != 0 || self.name.starts_with('.')
    }

    pub fn is_system(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_SYSTEM != 0
    }

    pub fn type_label(&self) -> &str {
        if self.is_dir {
            "Folder"
        } else if self.extension.is_empty() {
            "File"
        } else {
            &self.extension
        }
    }

    pub fn display_name(&self, show_extensions: bool) -> &str {
        if show_extensions || self.is_dir {
            &self.name
        } else {
            self.name
                .rfind('.')
                .filter(|index| *index > 0)
                .map(|index| &self.name[..index])
                .unwrap_or(&self.name)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Created,
    Accessed,
    Type,
}

pub fn sort_entries(entries: &mut [Entry], key: SortKey, descending: bool) {
    entries.sort_by(|a, b| {
        let dir_cmp = b.is_dir.cmp(&a.is_dir);
        if dir_cmp != Ordering::Equal {
            return dir_cmp;
        }
        let ord = match key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Created => a.created.cmp(&b.created),
            SortKey::Accessed => a.accessed.cmp(&b.accessed),
            SortKey::Type => a
                .extension
                .to_lowercase()
                .cmp(&b.extension.to_lowercase())
                .then_with(|| natural_cmp(&a.name, &b.name)),
        };
        if descending { ord.reverse() } else { ord }
    });
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ac), Some(bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let an = take_number(&mut ai);
                    let bn = take_number(&mut bi);
                    match an.cmp(&bn) {
                        Ordering::Equal => {}
                        ord => return ord,
                    }
                } else {
                    let ac = ai.next().unwrap().to_lowercase().next().unwrap_or('\0');
                    let bc = bi.next().unwrap().to_lowercase().next().unwrap_or('\0');
                    match ac.cmp(&bc) {
                        Ordering::Equal => {}
                        ord => return ord,
                    }
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u128 {
    let mut value = 0u128;
    while let Some(c) = it.peek() {
        if let Some(d) = c.to_digit(10) {
            value = value.saturating_mul(10).saturating_add(d as u128);
            it.next();
        } else {
            break;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: false,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
            attributes: 0,
            hidden: false,
            extension: String::new(),
        }
    }

    #[test]
    fn natural_sort_orders_numbers() {
        let mut items = vec![entry("file10"), entry("file2"), entry("file1")];
        sort_entries(&mut items, SortKey::Name, false);
        let names: Vec<_> = items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["file1", "file2", "file10"]);
    }

    #[test]
    fn directories_sort_first() {
        let mut folder = entry("z-folder");
        folder.is_dir = true;
        let mut items = vec![entry("a-file"), folder];
        sort_entries(&mut items, SortKey::Name, false);
        assert!(items[0].is_dir);
    }

    #[test]
    fn display_name_can_hide_extension() {
        let item = entry("report.pdf");
        assert_eq!(item.display_name(true), "report.pdf");
        assert_eq!(item.display_name(false), "report");
    }
}
