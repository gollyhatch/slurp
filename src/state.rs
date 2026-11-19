//! Reading position storage.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Saved position in one book.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookPosition {
    /// Canonical path of the book.
    pub path: String,
    /// Book title at the time of saving.
    #[serde(default)]
    pub title: String,
    /// Index of the chapter.
    #[serde(default)]
    pub chapter: usize,
    /// Scroll position as a fraction of the chapter height, from 0 to 1.
    #[serde(default)]
    pub progress: f32,
    /// Unix timestamp of the last update.
    #[serde(default)]
    pub updated: u64,
}

/// All saved positions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub books: Vec<BookPosition>,
}

impl Store {
    /// Read the store. A missing or broken file yields an empty store.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    /// Find the position for a path.
    pub fn get(&self, path: &str) -> Option<&BookPosition> {
        self.books.iter().find(|position| position.path == path)
    }

    /// Insert or replace a position.
    pub fn set(&mut self, position: BookPosition) {
        match self
            .books
            .iter_mut()
            .find(|existing| existing.path == position.path)
        {
            Some(existing) => *existing = position,
            None => self.books.push(position),
        }
        self.prune();
    }

    /// Write the store atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("cannot create {}", parent.display())
            })?;
        }
        let text = toml::to_string_pretty(self)
            .context("cannot serialize reading positions")?;
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    }

    fn prune(&mut self) {
        const LIMIT: usize = 200;
        if self.books.len() <= LIMIT {
            return;
        }
        self.books.sort_by_key(|position| position.updated);
        let excess = self.books.len() - LIMIT;
        self.books.drain(..excess);
    }
}

/// `$XDG_DATA_HOME/slurp/positions.toml` or `~/.local/share/slurp/positions.toml`.
pub fn default_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("slurp")
        .join("positions.toml")
}

/// Current unix timestamp in seconds.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_positions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.toml");

        let mut store = Store::default();
        store.set(BookPosition {
            path: "/books/a.epub".into(),
            title: "A".into(),
            chapter: 3,
            progress: 0.5,
            updated: 1,
        });
        store.save(&path).unwrap();

        let loaded = Store::load(&path);
        let position = loaded.get("/books/a.epub").unwrap();
        assert_eq!(position.chapter, 3);
        assert!((position.progress - 0.5).abs() < f32::EPSILON);
        assert_eq!(position.title, "A");
    }

    #[test]
    fn broken_file_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.toml");
        fs::write(&path, "not toml [").unwrap();
        assert!(Store::load(&path).books.is_empty());
    }

    #[test]
    fn replaces_existing_entry() {
        let mut store = Store::default();
        let position = BookPosition {
            path: "/a".into(),
            chapter: 1,
            ..Default::default()
        };
        store.set(position.clone());
        store.set(BookPosition {
            chapter: 2,
            ..position
        });
        assert_eq!(store.books.len(), 1);
        assert_eq!(store.books[0].chapter, 2);
    }
}
