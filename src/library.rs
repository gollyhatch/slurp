//! Recent book list shown when no book is given.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::state::{BookPosition, Store};
use crate::theme::Palette;

/// What the key handler asks the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryAction {
    /// Stay in the list.
    None,
    /// Open the book at this index.
    Open(usize),
    /// Leave the program.
    Quit,
}

/// The recent book list.
pub struct Library {
    store: Store,
    state_path: PathBuf,
    selected: usize,
    offset: usize,
    message: Option<String>,
    palette: Palette,
}

impl Library {
    /// Build the list from the stored positions, newest first.
    pub fn new(
        mut store: Store,
        palette: Palette,
        state_path: PathBuf,
    ) -> Self {
        store.books.sort_by_key(|book| std::cmp::Reverse(book.updated));
        Self {
            store,
            state_path,
            selected: 0,
            offset: 0,
            message: None,
            palette,
        }
    }

    /// Path of the selected book.
    pub fn selected_path(&self) -> Option<&str> {
        self.store
            .books
            .get(self.selected)
            .map(|book| book.path.as_str())
    }

    /// Show a message in place of the hint row.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Handle one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> LibraryAction {
        let last = self.store.books.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => LibraryAction::Quit,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                LibraryAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(last);
                LibraryAction::None
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(10);
                LibraryAction::None
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + 10).min(last);
                LibraryAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
                LibraryAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = last;
                LibraryAction::None
            }
            KeyCode::Enter if !self.store.books.is_empty() => {
                LibraryAction::Open(self.selected)
            }
            KeyCode::Char('x') if !self.store.books.is_empty() => {
                self.remove_selected();
                LibraryAction::None
            }
            KeyCode::Char('c') => {
                self.clean_missing();
                LibraryAction::None
            }
            _ => LibraryAction::None,
        }
    }

    /// Remove every book whose file is gone.
    fn clean_missing(&mut self) {
        let missing: Vec<String> = self
            .store
            .books
            .iter()
            .filter(|book| !Path::new(&book.path).exists())
            .map(|book| book.path.clone())
            .collect();
        if missing.is_empty() {
            self.message = Some("no missing books".to_string());
            return;
        }
        let count = missing.len();
        let mut store = self.store.clone();
        for path in &missing {
            store.remove(path);
        }
        let message = if count == 1 {
            "removed 1 missing book".to_string()
        } else {
            format!("removed {count} missing books")
        };
        self.apply_removal(store, message);
    }

    /// Remove the selected book from the list and the saved positions.
    fn remove_selected(&mut self) {
        let Some(book) = self.store.books.get(self.selected).cloned() else {
            return;
        };
        let label = if book.title.is_empty() {
            book.path.clone()
        } else {
            book.title.clone()
        };
        let mut store = self.store.clone();
        store.remove(&book.path);
        self.apply_removal(store, format!("removed {label}"));
    }

    /// Save the reduced store and show the outcome in the hint row.
    fn apply_removal(&mut self, store: Store, message: String) {
        if let Err(error) = store.save(&self.state_path) {
            self.message = Some(format!("cannot remove: {error:#}"));
            return;
        }
        self.store = store;
        let last = self.store.books.len().saturating_sub(1);
        self.selected = self.selected.min(last);
        self.offset = self.offset.min(last);
        self.message = Some(message);
    }

    /// Draw the list over the whole screen.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let block = Block::bordered()
            .title(" Recent books ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let [list_area, hint_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(inner);

        let rows = usize::from(list_area.height);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if rows > 0 && self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }

        let mut lines: Vec<Line> = Vec::new();
        if self.store.books.is_empty() {
            lines.push(Line::styled(
                " No recent books. Run: slurp BOOK.epub",
                self.palette.muted(),
            ));
        } else {
            for index in self.offset..self.offset.saturating_add(rows) {
                let Some(book) = self.store.books.get(index) else {
                    break;
                };
                lines.push(self.row(book, index == self.selected, list_area));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        let (hint, style) = match &self.message {
            Some(message) => (
                format!(" {message}"),
                self.palette.base().fg(self.palette.search),
            ),
            None => (
                " enter open   x remove   c clean missing   q quit "
                    .to_string(),
                self.palette.muted(),
            ),
        };
        frame.render_widget(
            Paragraph::new(Line::styled(hint, style)),
            hint_area,
        );
    }

    fn row(
        &self,
        book: &BookPosition,
        selected: bool,
        area: Rect,
    ) -> Line<'static> {
        let label = if book.title.is_empty() {
            book.path.clone()
        } else {
            book.title.clone()
        };
        let percent = (book.progress * 100.0).round().clamp(0.0, 100.0);
        let mut detail = format!("Chapter {}  {percent}%", book.chapter + 1);
        if book.chapters > 0 {
            let book_percent =
                (book.book_progress * 100.0).round().clamp(0.0, 100.0);
            detail.push_str(&format!("  Book {book_percent}%"));
        }
        let missing = !Path::new(&book.path).exists();
        let mut used = 1 + label.chars().count() + detail.chars().count();
        if missing {
            used += MISSING.len();
        }
        let padding = usize::from(area.width).saturating_sub(used).max(1);

        let label_style = if selected {
            self.palette.selected()
        } else {
            self.palette.accent()
        };
        let detail_style = if selected {
            self.palette.selected()
        } else {
            self.palette.muted()
        };
        let missing_style = if selected {
            self.palette.selected()
        } else {
            self.palette.base().fg(self.palette.search)
        };

        let mut spans = vec![
            Span::styled(format!(" {label}"), label_style),
            Span::styled(" ".repeat(padding), detail_style),
            Span::styled(detail, detail_style),
        ];
        if missing {
            spans.push(Span::styled(MISSING, missing_style));
        }
        Line::from(spans)
    }
}

/// Suffix that marks a book whose file is gone.
const MISSING: &str = "  [missing]";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Store;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn position(path: &str, title: &str, updated: u64) -> BookPosition {
        BookPosition {
            path: path.to_string(),
            title: title.to_string(),
            chapter: 1,
            progress: 0.5,
            updated,
            ..Default::default()
        }
    }

    fn screen(library: &mut Library) -> String {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| library.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn palette() -> Palette {
        crate::theme::load("nord", &std::collections::BTreeMap::new())
            .expect("built-in theme loads")
            .1
    }

    fn state_path() -> PathBuf {
        PathBuf::from("/nonexistent/slurp/positions.toml")
    }

    fn press(library: &mut Library, code: KeyCode) -> LibraryAction {
        library.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn sorts_newest_first_and_opens_the_selection() {
        let mut store = Store::default();
        store.books.push(position("/a.epub", "Oldest", 1));
        store.books.push(position("/b.epub", "Newest", 9));
        store.books.push(position("/c.epub", "Middle", 5));
        let mut library = Library::new(store, palette(), state_path());

        assert_eq!(library.selected_path(), Some("/b.epub"));
        let text = screen(&mut library);
        let newest = text.find("Newest").unwrap();
        let oldest = text.find("Oldest").unwrap();
        assert!(newest < oldest);

        assert_eq!(press(&mut library, KeyCode::Enter), LibraryAction::Open(0));
        press(&mut library, KeyCode::Down);
        assert_eq!(library.selected_path(), Some("/c.epub"));
        assert_eq!(press(&mut library, KeyCode::Down), LibraryAction::None);
        assert_eq!(press(&mut library, KeyCode::Enter), LibraryAction::Open(2));
        assert_eq!(
            press(&mut library, KeyCode::Char('q')),
            LibraryAction::Quit
        );
    }

    #[test]
    fn marks_missing_files() {
        let mut store = Store::default();
        store
            .books
            .push(position("/definitely/missing.epub", "Gone", 1));
        store.books.push(position("/tmp", "Present", 9));
        let palette = palette();
        let mut library = Library::new(store, palette.clone(), state_path());

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| library.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();

        let text: String =
            buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("[missing]"));
        let marker = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "[")
            .expect("missing marker is drawn");
        assert_eq!(marker.fg, palette.search);
    }

    #[test]
    fn uses_theme_colors() {
        let mut store = Store::default();
        store.books.push(position("/a.epub", "Only", 1));
        let palette = palette();
        let mut library = Library::new(store, palette.clone(), state_path());

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| library.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(0, 0)].fg, palette.border);
        assert_eq!(buffer[(0, 0)].bg, palette.background);
        let title = buffer[(1, 1)].clone();
        assert_eq!(title.symbol(), " ");
        let label = buffer[(2, 1)].clone();
        assert_eq!(label.bg, palette.selection);
        assert_eq!(label.fg, palette.accent);
    }

    #[test]
    fn shows_total_book_progress() {
        let mut store = Store::default();
        let mut book = position("/tmp", "Half", 5);
        book.chapter = 1;
        book.progress = 0.5;
        book.chapters = 4;
        book.book_progress = 0.375;
        store.books.push(book);
        let mut library = Library::new(store, palette(), state_path());

        let text = screen(&mut library);
        assert!(text.contains("Chapter 2  50%  Book 38%"), "row was {text}");
    }

    #[test]
    fn hides_book_progress_for_old_entries() {
        let mut store = Store::default();
        store.books.push(position("/tmp", "Old", 1));
        let mut library = Library::new(store, palette(), state_path());

        let text = screen(&mut library);
        assert!(text.contains("Chapter 2  50%"));
        assert!(!text.contains("Book "));
    }

    #[test]
    fn removes_the_selected_book_and_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("positions.toml");
        let mut store = Store::default();
        store.books.push(position("/a.epub", "Alpha", 1));
        store.books.push(position("/b.epub", "Beta", 9));
        store.save(&path).unwrap();

        let mut library =
            Library::new(Store::load(&path), palette(), path.clone());
        assert_eq!(library.selected_path(), Some("/b.epub"));
        assert_eq!(
            press(&mut library, KeyCode::Char('x')),
            LibraryAction::None
        );
        assert_eq!(library.selected_path(), Some("/a.epub"));
        assert!(screen(&mut library).contains("removed Beta"));
        let saved = Store::load(&path);
        assert!(saved.get("/b.epub").is_none());
        assert!(saved.get("/a.epub").is_some());

        press(&mut library, KeyCode::Char('x'));
        assert_eq!(library.selected_path(), None);
        assert!(screen(&mut library).contains("No recent books"));
        assert!(Store::load(&path).books.is_empty());
    }

    #[test]
    fn keeps_the_book_when_removal_cannot_be_saved() {
        let directory = tempfile::tempdir().unwrap();
        let blocker = directory.path().join("blocker");
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("positions.toml");
        let mut store = Store::default();
        store.books.push(position("/a.epub", "Alpha", 1));
        let mut library = Library::new(store, palette(), path);
        press(&mut library, KeyCode::Char('x'));
        assert_eq!(library.selected_path(), Some("/a.epub"));
        assert!(screen(&mut library).contains("cannot remove"));
    }

    #[test]
    fn cleans_out_missing_books() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("positions.toml");
        let present = directory.path().join("present.epub");
        std::fs::write(&present, "").unwrap();
        let present = present.to_string_lossy().into_owned();
        let mut store = Store::default();
        store.books.push(position(&present, "Present", 9));
        store
            .books
            .push(position("/definitely/missing-one.epub", "Gone One", 8));
        store
            .books
            .push(position("/definitely/missing-two.epub", "Gone Two", 7));
        store.save(&path).unwrap();

        let mut library =
            Library::new(Store::load(&path), palette(), path.clone());
        press(&mut library, KeyCode::Char('c'));
        assert_eq!(library.selected_path(), Some(present.as_str()));
        assert!(screen(&mut library).contains("removed 2 missing books"));
        let saved = Store::load(&path);
        assert_eq!(saved.books.len(), 1);
        assert!(saved.get(&present).is_some());
        assert!(saved.get("/definitely/missing-one.epub").is_none());
    }

    #[test]
    fn reports_when_nothing_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let present = directory.path().join("present.epub");
        std::fs::write(&present, "").unwrap();
        let present = present.to_string_lossy().into_owned();
        let mut store = Store::default();
        store.books.push(position(&present, "Present", 1));
        let mut library = Library::new(store, palette(), state_path());
        press(&mut library, KeyCode::Char('c'));
        assert_eq!(library.selected_path(), Some(present.as_str()));
        assert!(screen(&mut library).contains("no missing books"));
    }

    #[test]
    fn shows_the_empty_hint_and_ignores_enter() {
        let palette = palette();
        let mut library = Library::new(
            Store::default(),
            palette.clone(),
            state_path(),
        );
        assert_eq!(library.selected_path(), None);
        assert_eq!(press(&mut library, KeyCode::Enter), LibraryAction::None);

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| library.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String =
            buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("No recent books"));
        assert!(text.contains("Recent books"));
        assert!(text.contains("x remove"));
        assert!(text.contains("c clean missing"));
        assert_eq!(buffer[(2, 1)].fg, palette.muted);
        let hint = buffer[(1, 18)].clone();
        assert_eq!(hint.fg, palette.muted);

        library.set_message("cannot open");
        terminal.draw(|frame| library.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String =
            buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("cannot open"));
        assert_eq!(buffer[(2, 18)].fg, palette.search);
    }
}
