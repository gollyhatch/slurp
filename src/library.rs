//! Recent book list shown when no book is given.

use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::state::{BookPosition, Store};

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
    books: Vec<BookPosition>,
    selected: usize,
    offset: usize,
    message: Option<String>,
}

impl Library {
    /// Build the list from the stored positions, newest first.
    pub fn new(store: Store) -> Self {
        let mut books = store.books;
        books.sort_by_key(|book| std::cmp::Reverse(book.updated));
        Self {
            books,
            selected: 0,
            offset: 0,
            message: None,
        }
    }

    /// Path of the selected book.
    pub fn selected_path(&self) -> Option<&str> {
        self.books.get(self.selected).map(|book| book.path.as_str())
    }

    /// Show a message in place of the hint row.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Handle one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> LibraryAction {
        let last = self.books.len().saturating_sub(1);
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
            KeyCode::Enter if !self.books.is_empty() => {
                LibraryAction::Open(self.selected)
            }
            _ => LibraryAction::None,
        }
    }

    /// Draw the list over the whole screen.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let block = Block::bordered().title(" Recent books ");
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
        if self.books.is_empty() {
            lines.push(Line::from(" No recent books. Run: slurp BOOK.epub"));
        } else {
            for index in self.offset..self.offset.saturating_add(rows) {
                let Some(book) = self.books.get(index) else {
                    break;
                };
                lines.push(self.row(book, index == self.selected, list_area));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        let hint = match &self.message {
            Some(message) => format!(" {message}"),
            None => " enter open   q quit ".to_string(),
        };
        let style = if self.message.is_some() {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
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
        if !Path::new(&book.path).exists() {
            detail.push_str("  [missing]");
        }
        let width = usize::from(area.width);
        let used = label.chars().count() + detail.chars().count() + 1;
        let padding = width.saturating_sub(used).max(1);
        let style = if selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::styled(format!(" {label}{}{detail}", " ".repeat(padding)), style)
    }
}

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

    fn press(library: &mut Library, code: KeyCode) -> LibraryAction {
        library.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn sorts_newest_first_and_opens_the_selection() {
        let mut store = Store::default();
        store.books.push(position("/a.epub", "Oldest", 1));
        store.books.push(position("/b.epub", "Newest", 9));
        store.books.push(position("/c.epub", "Middle", 5));
        let mut library = Library::new(store);

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
        let mut library = Library::new(store);

        assert!(screen(&mut library).contains("[missing]"));
    }

    #[test]
    fn shows_the_empty_hint_and_ignores_enter() {
        let mut library = Library::new(Store::default());
        assert_eq!(library.selected_path(), None);
        assert_eq!(press(&mut library, KeyCode::Enter), LibraryAction::None);
        let text = screen(&mut library);
        assert!(text.contains("No recent books"));
        assert!(text.contains("Recent books"));

        library.set_message("cannot open");
        assert!(screen(&mut library).contains("cannot open"));
    }
}
