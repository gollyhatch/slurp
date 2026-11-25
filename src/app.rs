//! Application state, input handling, and drawing.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::book::{Book, LinkTarget};
use crate::config::Config;
use crate::layout::{self, Options, View};
use crate::state::{self, BookPosition, Bookmark, Store};
use crate::sync::{self, RemoteProgress, SyncClient};
use crate::theme::{self, Palette};

/// Maximum number of search hits kept in memory.
const MAX_HITS: usize = 5000;

/// Input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Percent,
    Toc,
    Help,
    Theme,
    Bookmarks,
    BookmarkLabel,
    Stats,
}

/// One search result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub chapter: usize,
    pub block: usize,
    pub offset: usize,
}

/// State of a full-text search.
#[derive(Debug, Clone)]
pub struct Search {
    pub query: String,
    pub hits: Vec<Hit>,
    pub current: usize,
}

/// The reader.
pub struct App {
    book: Book,
    config: Config,
    palette: Palette,
    picker: Picker,
    view: View,
    view_cache: HashMap<usize, View>,
    chapter: usize,
    scroll: usize,
    content_width: u16,
    viewport_rows: u16,
    area: Rect,
    laid_out: bool,
    mode: Mode,
    input: String,
    search: Option<Search>,
    search_before: Option<Search>,
    selected_link: Option<usize>,
    link_history: Vec<(usize, usize)>,
    bookmarks: Vec<Bookmark>,
    bookmark_input: String,
    bookmark_selected: usize,
    bookmark_offset: usize,
    toc_selected: usize,
    toc_offset: usize,
    toc_matches: Vec<usize>,
    toc_filter: String,
    toc_list_area: Option<Rect>,
    theme_matches: Vec<usize>,
    theme_filter: String,
    theme_selected: usize,
    theme_offset: usize,
    theme_list_area: Option<Rect>,
    theme_before: Option<(String, Palette)>,
    images: bool,
    message: Option<String>,
    quit: bool,
    image_dimensions: HashMap<String, (u32, u32)>,
    image_protocols: HashMap<(String, u16, u16), Protocol>,
    state: Store,
    state_path: PathBuf,
    book_key: String,
    sync: Option<SyncClient>,
    sync_document: String,
    sync_ready: bool,
    pending_progress: Option<f32>,
    chapter_chars: Vec<usize>,
    total_chars: usize,
    chapter_words: Vec<usize>,
    /// Indicator color offset of each chapter, in paragraphs.
    chapter_paragraph_offsets: Vec<usize>,
    auto_scroll: bool,
    auto_scroll_wpm: u16,
    auto_scroll_accumulator: f64,
    auto_scroll_idle: f64,
    last_tick: Instant,
}

impl App {
    /// Open a book and prepare the reader.
    pub fn new(
        book_path: &Path,
        config: Config,
        picker: Picker,
        state_path: PathBuf,
    ) -> Result<Self> {
        let book = Book::open(book_path)?;
        let (_theme, palette) = theme::load(&config.theme, &config.colors)?;
        let chapter_chars: Vec<usize> = book
            .chapters
            .iter()
            .map(|chapter| {
                chapter.plain.iter().map(|text| text.chars().count()).sum()
            })
            .collect();
        let total_chars = chapter_chars.iter().sum();
        let chapter_words: Vec<usize> = book
            .chapters
            .iter()
            .map(|chapter| {
                chapter
                    .plain
                    .iter()
                    .map(|text| text.split_whitespace().count())
                    .sum()
            })
            .collect();

        let book_key = fs::canonicalize(book_path)
            .unwrap_or_else(|_| book_path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let mut chapter_paragraph_offsets =
            Vec::with_capacity(book.chapters.len());
        let mut offset = 0;
        for chapter in &book.chapters {
            chapter_paragraph_offsets.push(offset);
            offset += layout::paragraph_count(chapter);
        }
        let mut sync_error = None;
        let sync = if config.sync.enabled {
            match SyncClient::new(&config.sync) {
                Ok(client) => {
                    match sync::document_id(
                        Path::new(&book_key),
                        config.sync.document_id,
                    ) {
                        Ok(document) => Some((client, document)),
                        Err(error) => {
                            sync_error = Some(format!("{error:#}"));
                            None
                        }
                    }
                }
                Err(error) => {
                    sync_error = Some(format!("{error:#}"));
                    None
                }
            }
        } else {
            None
        };
        let (sync, sync_document) = match sync {
            Some((client, document)) => (Some(client), document),
            None => (None, String::new()),
        };
        let store = Store::load(&state_path);
        let saved = store.get(&book_key).cloned();
        let bookmarks = saved
            .as_ref()
            .map(|position| position.bookmarks.clone())
            .unwrap_or_default();

        let chapter = saved
            .as_ref()
            .map(|position| position.chapter)
            .unwrap_or(0)
            .min(book.chapters.len() - 1);

        let images = config.images;
        let content_width = config.text_width;
        let auto_scroll_wpm = config.auto_scroll_wpm;
        let mut app = Self {
            book,
            config,
            images,
            content_width,
            viewport_rows: 24,
            area: Rect::new(0, 0, 80, 24),
            palette,
            picker,
            view: View::empty(),
            view_cache: HashMap::new(),
            chapter,
            scroll: 0,
            laid_out: false,
            mode: Mode::Normal,
            input: String::new(),
            search: None,
            search_before: None,
            selected_link: None,
            link_history: Vec::new(),
            bookmarks,
            bookmark_input: String::new(),
            bookmark_selected: 0,
            bookmark_offset: 0,
            toc_selected: 0,
            toc_offset: 0,
            toc_matches: Vec::new(),
            toc_filter: String::new(),
            toc_list_area: None,
            theme_matches: Vec::new(),
            theme_filter: String::new(),
            theme_selected: 0,
            theme_offset: 0,
            theme_list_area: None,
            theme_before: None,
            message: None,
            quit: false,
            image_dimensions: HashMap::new(),
            image_protocols: HashMap::new(),
            state: store,
            state_path,
            book_key,
            sync,
            sync_document,
            sync_ready: false,
            pending_progress: saved.map(|position| position.progress),
            chapter_chars,
            total_chars,
            chapter_words,
            chapter_paragraph_offsets,
            auto_scroll: false,
            auto_scroll_wpm,
            auto_scroll_accumulator: 0.0,
            auto_scroll_idle: 0.0,
            last_tick: Instant::now(),
        };
        if let Some(error) = sync_error {
            app.message = Some(format!("sync: {error}"));
        }
        app.pull_progress();
        app.rebuild_view();
        Ok(app)
    }

    /// Current chapter index.
    pub fn chapter(&self) -> usize {
        self.chapter
    }

    /// Current input mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Identifier of the applied theme.
    pub fn theme_name(&self) -> &str {
        &self.config.theme
    }

    /// Current search, if a search was run.
    pub fn search(&self) -> Option<&Search> {
        self.search.as_ref()
    }

    /// Laid out chapter.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Scroll position in rows.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Save the current position.
    pub fn save_position(&mut self) -> Result<()> {
        let progress = if self.view.total_rows > 0 {
            (self.scroll as f32 / self.view.total_rows as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let book_progress = self.book_fraction();
        self.state.set(BookPosition {
            path: self.book_key.clone(),
            title: self.book.title.clone(),
            chapter: self.chapter,
            progress,
            updated: state::now(),
            bookmarks: self.bookmarks.clone(),
            chapters: self.book.chapters.len(),
            book_progress,
        });
        self.state.save(&self.state_path)
    }

    /// Reading progress in the whole book, from 0 to 1.
    fn book_fraction(&self) -> f32 {
        if self.total_chars == 0 || self.book.chapters.is_empty() {
            return 0.0;
        }
        let before: usize = self.chapter_chars.iter().take(self.chapter).sum();
        let chars = self.chapter_chars.get(self.chapter).copied().unwrap_or(0);
        let local = if self.view.total_rows > 0 {
            (self.scroll as f32 / self.view.total_rows as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ((before as f32 + local * chars as f32) / self.total_chars as f32)
            .clamp(0.0, 1.0)
    }

    /// Fetch the remote position and adopt it when it is newer than the
    /// local one.
    fn pull_progress(&mut self) {
        let remote = {
            let Some(client) = &self.sync else {
                return;
            };
            client.pull(&self.sync_document)
        };
        let remote = match remote {
            Ok(Some(remote)) => remote,
            Ok(None) => {
                self.sync_ready = true;
                return;
            }
            Err(error) => {
                self.message = Some(format!("sync: {error:#}"));
                return;
            }
        };
        self.sync_ready = true;

        let saved = self.state.get(&self.book_key).cloned();
        let newer = saved
            .as_ref()
            .map(|saved| remote.timestamp > saved.updated)
            .unwrap_or(true);
        if !newer {
            return;
        }
        let local = saved
            .as_ref()
            .map(|saved| {
                if saved.chapters > 0 {
                    saved.book_progress
                } else {
                    book_fraction_at(
                        &self.chapter_chars,
                        self.total_chars,
                        saved.chapter,
                        saved.progress,
                    )
                }
            })
            .unwrap_or(0.0);
        if (remote.percentage - local).abs() >= 0.001 {
            self.adopt_remote(&remote);
        }
    }

    /// Fetch the remote position on demand and jump to it.
    fn pull_latest(&mut self) {
        if self.sync.is_none() {
            self.message = Some("sync is not enabled".to_string());
            return;
        }
        let remote = {
            let Some(client) = &self.sync else {
                return;
            };
            client.pull(&self.sync_document)
        };
        match remote {
            Ok(Some(remote)) => {
                self.sync_ready = true;
                self.jump_to_remote(&remote);
            }
            Ok(None) => {
                self.sync_ready = true;
                self.message = Some("sync: no remote progress".to_string());
            }
            Err(error) => self.message = Some(format!("sync: {error:#}")),
        }
    }

    /// Jump to a whole-book fraction from the sync server.
    fn adopt_remote(&mut self, remote: &RemoteProgress) {
        let (chapter, progress) = self.locate_fraction(remote.percentage);
        self.chapter = chapter;
        self.selected_link = None;
        self.view_cache.clear();
        self.pending_progress = Some(progress);
        self.message = Some(remote_label(remote));
    }

    /// Jump to a remote position after the view has been laid out.
    fn jump_to_remote(&mut self, remote: &RemoteProgress) {
        let (chapter, progress) = self.locate_fraction(remote.percentage);
        self.chapter = chapter.min(self.book.chapters.len().saturating_sub(1));
        self.selected_link = None;
        self.view_cache.clear();
        self.rebuild_view();
        let row = (progress * self.view.total_rows as f32).round() as usize;
        self.scroll = row.min(self.scroll_limit());
        self.message = Some(remote_label(remote));
        let _ = self.save_position();
    }

    /// Chapter and local progress for a whole-book fraction.
    fn locate_fraction(&self, fraction: f32) -> (usize, f32) {
        if self.total_chars == 0 || self.chapter_chars.is_empty() {
            return (0, 0.0);
        }
        let target =
            (fraction.clamp(0.0, 1.0) * self.total_chars as f32).round()
                as usize;
        let mut before = 0;
        for (index, &chars) in self.chapter_chars.iter().enumerate() {
            let end = before + chars;
            if target < end || index + 1 == self.chapter_chars.len() {
                let local = if chars == 0 {
                    0.0
                } else {
                    (target.saturating_sub(before) as f32 / chars as f32)
                        .clamp(0.0, 1.0)
                };
                return (index, local);
            }
            before = end;
        }
        (self.chapter_chars.len().saturating_sub(1), 0.0)
    }

    /// Push the current position after a successful pull in this session.
    pub fn push_sync(&self) -> Result<()> {
        if !self.sync_ready {
            return Ok(());
        }
        let Some(client) = &self.sync else {
            return Ok(());
        };
        let filename = Path::new(&self.book_key)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        client.push(
            &self.sync_document,
            self.book_fraction(),
            &self.book.title,
            self.book.author.as_deref(),
            filename,
        )
    }

    /// Handle one key press. Returns `true` when the reader must exit.
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c'))
        {
            self.quit = true;
            self.save_position()?;
            return Ok(true);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        {
            if self.mode != Mode::Theme {
                self.open_theme_selector();
            }
            return Ok(false);
        }

        self.message = None;
        match self.mode {
            Mode::Normal => self.key_normal(key),
            Mode::Search => self.key_search(key),
            Mode::Percent => self.key_percent(key),
            Mode::Toc => self.key_toc(key),
            Mode::Theme => self.key_theme(key),
            Mode::Bookmarks => self.key_bookmarks(key),
            Mode::BookmarkLabel => self.key_bookmark_label(key),
            Mode::Stats => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s')
                ) {
                    self.mode = Mode::Normal;
                }
            }
            Mode::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc
                        | KeyCode::Char('q')
                        | KeyCode::Char('?')
                        | KeyCode::Enter
                ) {
                    self.mode = Mode::Normal;
                }
            }
        }
        Ok(self.quit)
    }

    /// Handle one mouse event. A single left click activates a popup entry.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollUp => match self.mode {
                Mode::Normal => self.scroll_by(-3),
                Mode::Toc => {
                    self.toc_selected = self.toc_selected.saturating_sub(1)
                }
                Mode::Theme => self.select_theme(-1),
                Mode::Bookmarks => {
                    self.bookmark_selected =
                        self.bookmark_selected.saturating_sub(1)
                }
                _ => {}
            },
            MouseEventKind::ScrollDown => match self.mode {
                Mode::Normal => self.scroll_by(3),
                Mode::Toc => {
                    self.toc_selected = (self.toc_selected + 1)
                        .min(self.toc_matches.len().saturating_sub(1))
                }
                Mode::Theme => self.select_theme(1),
                Mode::Bookmarks if !self.bookmarks.is_empty() => {
                    self.bookmark_selected = (self.bookmark_selected + 1)
                        .min(self.bookmarks.len() - 1);
                }
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Left) => {
                let position = Position::new(event.column, event.row);
                match self.mode {
                    Mode::Toc => {
                        let Some(area) = self.toc_list_area else {
                            return;
                        };
                        if !area.contains(position) {
                            return;
                        }
                        let index =
                            self.toc_offset + usize::from(event.row - area.y);
                        if index < self.toc_matches.len() {
                            self.toc_selected = index;
                            self.activate_toc();
                        }
                    }
                    Mode::Theme => {
                        let Some(area) = self.theme_list_area else {
                            return;
                        };
                        if !area.contains(position) {
                            return;
                        }
                        let index =
                            self.theme_offset + usize::from(event.row - area.y);
                        if index < self.theme_matches.len() {
                            self.theme_selected = index;
                            self.preview_theme();
                            self.apply_theme();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Draw the reader.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let [body, status_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(area);
        let body_area = self.inset_body(body);
        self.ensure_size(body_area);

        if let Some((_, top, rows)) = self.view.item_at_row(self.scroll) {
            if rows > 1 && top < self.scroll {
                self.scroll = top;
            }
        }
        frame.render_widget(
            Block::new().style(Style::default().bg(self.palette.background)),
            area,
        );

        self.draw_body(frame, body_area);
        self.draw_footer(frame, status_area);

        match self.mode {
            Mode::Toc => self.draw_toc(frame, area),
            Mode::Help => self.draw_help(frame, area),
            Mode::Theme => self.draw_theme_selector(frame, area),
            Mode::Bookmarks => self.draw_bookmarks(frame, area),
            Mode::Stats => self.draw_stats(frame, area),
            _ => {}
        }

        let prompt = match self.mode {
            Mode::Search => Some(format!("/ {}", self.input)),
            Mode::Percent => Some(format!("goto {}", self.input)),
            Mode::BookmarkLabel => {
                Some(format!("bookmark: {}", self.bookmark_input))
            }
            _ => None,
        };
        if let Some(prompt) = prompt {
            let x = status_area
                .x
                .saturating_add(1)
                .saturating_add(layout::display_width(&prompt) as u16);
            if x < status_area.right() {
                frame.set_cursor_position(Position::new(x, status_area.y));
            }
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('o')
        {
            self.go_back_link();
            return;
        }

        let half = (self.viewport_rows / 2).max(1) as i64;
        let page = self.viewport_rows.max(1) as i64;

        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                let _ = self.save_position();
            }
            KeyCode::Tab => self.select_next_link(),
            KeyCode::Enter => self.follow_selected_link(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('d') => self.scroll_by(half),
            KeyCode::Char('u') => self.scroll_by(-half),
            KeyCode::Char(' ') => self.next_paragraph(),
            KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => {
                if self.config.continuous && self.chapter != 0 {
                    self.switch_chapter(0);
                }
                self.scroll = 0;
            }
            KeyCode::Char('G') | KeyCode::End => {
                if self.config.continuous {
                    let last = self.book.chapters.len() - 1;
                    if self.chapter != last {
                        self.switch_chapter(last);
                    }
                }
                self.scroll =
                    self.snap(self.max_scroll(), true).min(self.max_scroll());
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.open_chapter(self.chapter.saturating_sub(1))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.open_chapter(
                    (self.chapter + 1).min(self.book.chapters.len() - 1),
                );
            }
            KeyCode::Char('t') => {
                self.toc_filter.clear();
                self.toc_matches = (0..self.book.toc.len()).collect();
                self.toc_selected = self
                    .book
                    .toc
                    .iter()
                    .rposition(|entry| entry.chapter <= self.chapter)
                    .unwrap_or(0)
                    .min(self.toc_matches.len().saturating_sub(1));
                self.toc_offset = 0;
                self.mode = Mode::Toc;
            }
            KeyCode::Char('m') => {
                self.bookmark_input.clear();
                self.mode = Mode::BookmarkLabel;
            }
            KeyCode::Char('\'') => {
                self.bookmark_selected = self
                    .bookmarks
                    .iter()
                    .rposition(|bookmark| bookmark.chapter <= self.chapter)
                    .unwrap_or(0);
                self.bookmark_offset = 0;
                self.mode = Mode::Bookmarks;
            }
            KeyCode::Char('s') => self.mode = Mode::Stats,
            KeyCode::Char('S') => self.save_settings(),
            KeyCode::Char('%') => {
                self.input.clear();
                self.mode = Mode::Percent;
            }
            KeyCode::Char('/') => {
                self.search_before = self.search.clone();
                self.input.clear();
                self.mode = Mode::Search;
            }
            KeyCode::Char('n') => self.next_match(true),
            KeyCode::Char('N') => self.next_match(false),
            KeyCode::Char('i') => {
                self.images = !self.images;
                self.rebuild_view();
                self.message = Some(format!(
                    "images: {}",
                    if self.images { "on" } else { "off" }
                ));
            }
            KeyCode::Char('c') => {
                self.config.continuous = !self.config.continuous;
                self.rebuild_view();
                self.message = Some(format!(
                    "continuous: {}",
                    if self.config.continuous { "on" } else { "off" }
                ));
            }
            KeyCode::Char('p') => {
                self.config.paragraph_colors = !self.config.paragraph_colors;
                self.message = Some(format!(
                    "paragraph colors: {}",
                    if self.config.paragraph_colors { "on" } else { "off" }
                ));
            }
            KeyCode::Char('r') => self.pull_latest(),
            KeyCode::Char('a') => self.toggle_auto_scroll(),
            KeyCode::Char('[') => self.adjust_auto_scroll_speed(-25),
            KeyCode::Char(']') => self.adjust_auto_scroll_speed(25),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_width(4),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_width(-4),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Esc => {
                self.selected_link = None;
                self.search = None;
                self.rebuild_view();
            }
            _ => {}
        }
    }

    fn key_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search = self.search_before.take();
                self.rebuild_view();
            }
            KeyCode::Enter => self.run_search(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.input.push(ch);
            }
            _ => {}
        }
    }

    fn key_percent(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.input.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.input);
                self.mode = Mode::Normal;
                match text.trim().parse::<u32>() {
                    Ok(percent) => self.jump_to_percent(percent.min(100) as u8),
                    Err(_) => {
                        self.message =
                            Some("enter a number from 0 to 100".to_string())
                    }
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(ch)
                if ch.is_ascii_digit()
                    && self.input.len() < 3
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.input.push(ch);
            }
            _ => {}
        }
    }

    fn jump_to_percent(&mut self, percent: u8) {
        if self.book.chapters.is_empty() || self.total_chars == 0 {
            self.message = Some("book has no text".to_string());
            return;
        }
        let percent = percent.min(100) as usize;
        let target = (self.total_chars * percent + 50) / 100;

        let mut accumulated = 0;
        let mut chapter = self.book.chapters.len() - 1;
        let mut fraction = 1.0;
        for (index, &chars) in self.chapter_chars.iter().enumerate() {
            let end = accumulated + chars;
            if target < end || index == self.book.chapters.len() - 1 {
                fraction = if chars > 0 {
                    target.saturating_sub(accumulated).min(chars) as f32
                        / chars as f32
                } else {
                    0.0
                };
                chapter = index;
                break;
            }
            accumulated = end;
        }

        self.selected_link = None;
        self.chapter = chapter;
        self.scroll = 0;
        self.rebuild_view();
        let limit = self.scroll_limit();
        let row = (fraction * self.view.total_rows as f32).round() as usize;
        self.scroll = self.snap(row.min(limit), true).min(limit);
        let _ = self.save_position();
    }

    /// Approximate position in the whole book, from 0 to 100.
    pub fn book_percent(&self) -> u8 {
        if self.total_chars == 0 || self.book.chapters.is_empty() {
            return 0;
        }
        if self.chapter + 1 == self.book.chapters.len()
            && self.scroll >= self.max_scroll()
        {
            return 100;
        }
        let before: usize = self.chapter_chars.iter().take(self.chapter).sum();
        let chars = self.chapter_chars.get(self.chapter).copied().unwrap_or(0);
        let local = if self.view.total_rows > 0 {
            (self.scroll as f32 / self.view.total_rows as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let inside = (local * chars as f32).round() as usize;
        (((before + inside) * 100) / self.total_chars).min(100) as u8
    }

    /// Move the link selection to the next link of the chapter.
    fn select_next_link(&mut self) {
        let Some(chapter) = self.book.chapters.get(self.chapter) else {
            return;
        };
        let count = chapter.links.len();
        if count == 0 {
            self.message = Some("no links in this chapter".to_string());
            return;
        }
        let next = self
            .selected_link
            .map(|index| (index + 1) % count)
            .unwrap_or(0);
        let link = &chapter.links[next];
        let block = link.block;
        let label = if link.label.is_empty() {
            link.href.clone()
        } else {
            link.label.clone()
        };

        self.selected_link = Some(next);
        if let Some(item) = self.view.block_first_item.get(block).copied() {
            self.scroll = self.view.tops[item].min(self.scroll_limit());
        }
        self.message = Some(format!("link {}/{}: {label}", next + 1, count));
    }

    /// Open the selected link.
    fn follow_selected_link(&mut self) {
        let Some(link) = self
            .selected_link
            .and_then(|index| self.book.chapters[self.chapter].links.get(index))
            .cloned()
        else {
            return;
        };

        match self.book.resolve_link(self.chapter, &link.href) {
            LinkTarget::Chapter { chapter, block } => {
                self.link_history.push((self.chapter, self.scroll));
                if self.link_history.len() > 64 {
                    self.link_history.remove(0);
                }
                self.chapter = chapter.min(self.book.chapters.len() - 1);
                self.selected_link = None;
                self.rebuild_view();
                if let Some(block) = block {
                    if let Some(item) =
                        self.view.block_first_item.get(block).copied()
                    {
                        self.scroll =
                            self.view.tops[item].min(self.scroll_limit());
                    }
                }
            }
            LinkTarget::External => {
                self.message = Some(format!("external link: {}", link.href));
            }
            LinkTarget::Missing => {
                self.message =
                    Some(format!("link target not found: {}", link.href));
            }
        }
    }

    /// Return to the position before the last followed link.
    fn go_back_link(&mut self) {
        let Some((chapter, scroll)) = self.link_history.pop() else {
            self.message = Some("no link history".to_string());
            return;
        };
        self.chapter = chapter.min(self.book.chapters.len() - 1);
        self.selected_link = None;
        self.rebuild_view();
        self.scroll = scroll.min(self.scroll_limit());
        let _ = self.save_position();
    }

    fn key_toc(&mut self, key: KeyEvent) {
        let last = self.toc_matches.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Down => {
                self.toc_selected = (self.toc_selected + 1).min(last);
            }
            KeyCode::Up => {
                self.toc_selected = self.toc_selected.saturating_sub(1);
            }
            KeyCode::Home => self.toc_selected = 0,
            KeyCode::End => self.toc_selected = last,
            KeyCode::PageDown => {
                self.toc_selected = (self.toc_selected + 10).min(last);
            }
            KeyCode::PageUp => {
                self.toc_selected = self.toc_selected.saturating_sub(10);
            }
            KeyCode::Enter => self.activate_toc(),
            KeyCode::Backspace => {
                self.toc_filter.pop();
                self.refresh_toc_matches();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.toc_filter.push(ch);
                self.refresh_toc_matches();
            }
            _ => {}
        }
    }

    /// Open the selected table of contents entry.
    fn activate_toc(&mut self) {
        if let Some(&index) = self.toc_matches.get(self.toc_selected) {
            if let Some(entry) = self.book.toc.get(index) {
                let chapter = entry.chapter;
                self.mode = Mode::Normal;
                self.open_chapter(chapter);
            }
        }
    }

    fn refresh_toc_matches(&mut self) {
        if self.toc_filter.is_empty() {
            self.toc_matches = (0..self.book.toc.len()).collect();
        } else {
            let matcher = SkimMatcherV2::default().ignore_case();
            let mut scored: Vec<(i64, usize)> = self
                .book
                .toc
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    matcher
                        .fuzzy_match(&entry.title, &self.toc_filter)
                        .map(|score| (score, index))
                })
                .collect();
            scored.sort_by(|left, right| {
                right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1))
            });
            self.toc_matches =
                scored.into_iter().map(|(_, index)| index).collect();
        }
        self.toc_selected = 0;
    }

    fn open_chapter(&mut self, index: usize) {
        let index = index.min(self.book.chapters.len() - 1);
        if index == self.chapter {
            return;
        }
        self.selected_link = None;
        self.chapter = index;
        self.scroll = 0;
        self.rebuild_view();
        let _ = self.save_position();
    }

    fn toggle_auto_scroll(&mut self) {
        self.auto_scroll = !self.auto_scroll;
        self.auto_scroll_accumulator = 0.0;
        self.auto_scroll_idle = 0.0;
        self.last_tick = Instant::now();
        self.message = if self.auto_scroll {
            None
        } else {
            Some("auto-scroll: off".to_string())
        };
    }

    fn adjust_auto_scroll_speed(&mut self, delta: i32) {
        let wpm =
            (i32::from(self.auto_scroll_wpm) + delta).clamp(25, 1000) as u16;
        if wpm != self.auto_scroll_wpm {
            self.auto_scroll_wpm = wpm;
            self.message = if self.auto_scroll {
                None
            } else {
                Some(format!("auto-scroll: {wpm} wpm"))
            };
        }
    }

    /// Wait time used by the event loop.
    pub fn poll_timeout(&self) -> Duration {
        Duration::from_millis(if self.auto_scroll { 100 } else { 250 })
    }

    /// Advance auto-scroll for the time since the last tick.
    pub fn tick(&mut self) {
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_tick)
            .min(Duration::from_millis(500));
        self.last_tick = now;
        self.advance_auto_scroll(elapsed);
    }

    fn advance_auto_scroll(&mut self, elapsed: Duration) {
        if !self.auto_scroll || self.mode != Mode::Normal {
            return;
        }

        if self.config.continuous {
            if self.chapter + 1 == self.book.chapters.len()
                && self.scroll >= self.max_scroll()
            {
                self.auto_scroll = false;
                self.message = Some("auto-scroll: end of book".to_string());
                return;
            }
        } else {
            let max = self.max_scroll();
            if self.scroll >= max {
                self.auto_scroll_idle += elapsed.as_secs_f64();
                if self.auto_scroll_idle < self.end_of_chapter_delay() {
                    return;
                }
                self.auto_scroll_idle = 0.0;
                if self.chapter + 1 < self.book.chapters.len() {
                    self.open_chapter(self.chapter + 1);
                } else {
                    self.auto_scroll = false;
                    self.message = Some("auto-scroll: end of book".to_string());
                }
                return;
            }
        }

        let rate = self.auto_scroll_rows_per_second();
        if rate <= 0.0 {
            return;
        }
        self.auto_scroll_accumulator += rate * elapsed.as_secs_f64();
        let rows = self.auto_scroll_accumulator.floor() as i64;
        if rows > 0 {
            self.auto_scroll_accumulator -= rows as f64;
            self.scroll_by(rows);
        }
    }

    fn auto_scroll_rows_per_second(&self) -> f64 {
        let words = self.chapter_words.get(self.chapter).copied().unwrap_or(0);
        let rows = self.view.total_rows.max(1);
        if words == 0 {
            return 0.0;
        }
        let words_per_row = words as f64 / rows as f64;
        if words_per_row <= 0.0 {
            return 0.0;
        }
        let rows_per_minute = f64::from(self.auto_scroll_wpm) / words_per_row;
        (rows_per_minute / 60.0).clamp(0.0, 20.0)
    }

    fn end_of_chapter_delay(&self) -> f64 {
        if self.view.total_rows <= usize::from(self.viewport_rows) {
            let words =
                self.chapter_words.get(self.chapter).copied().unwrap_or(0)
                    as f64;
            let wpm = f64::from(self.auto_scroll_wpm).max(1.0);
            (words / (wpm / 60.0)).clamp(0.5, 15.0)
        } else {
            0.8
        }
    }

    fn next_paragraph(&mut self) {
        match self.view.next_paragraph_row(self.scroll) {
            Some(top) => self.scroll = top.min(self.scroll_limit()),
            None if self.config.continuous
                && self.chapter + 1 < self.book.chapters.len() =>
            {
                self.switch_chapter(self.chapter + 1);
            }
            None => self.scroll = self.max_scroll(),
        }
    }

    fn scroll_by(&mut self, delta: i64) {
        if self.config.continuous {
            self.scroll_continuous(delta);
            return;
        }
        if self.view.total_rows <= usize::from(self.viewport_rows) {
            self.scroll = 0;
            return;
        }
        let max = self.max_scroll() as i64;
        let target = (self.scroll as i64 + delta).clamp(0, max) as usize;
        self.scroll = self.snap(target, delta >= 0).min(max as usize);
    }

    fn scroll_continuous(&mut self, delta: i64) {
        let start_chapter = self.chapter;
        if delta > 0 {
            let mut remaining = usize::try_from(delta).unwrap_or(usize::MAX);
            loop {
                let rows = self.view.total_rows;
                let available = rows.saturating_sub(self.scroll);
                if remaining < available {
                    self.scroll += remaining;
                    break;
                }
                remaining -= available;
                if self.chapter + 1 < self.book.chapters.len() {
                    self.switch_chapter(self.chapter + 1);
                    if remaining == 0 {
                        break;
                    }
                } else {
                    self.scroll = self.max_scroll();
                    break;
                }
            }
        } else if delta < 0 {
            let mut back = usize::try_from(-delta).unwrap_or(usize::MAX);
            loop {
                if back <= self.scroll {
                    self.scroll -= back;
                    break;
                }
                back -= self.scroll;
                if self.chapter == 0 {
                    self.scroll = 0;
                    break;
                }
                self.switch_chapter(self.chapter - 1);
                let rows = self.view.total_rows;
                if rows == 0 {
                    self.scroll = 0;
                    continue;
                }
                if back <= rows {
                    self.scroll = rows - back;
                    break;
                }
                back -= rows;
                self.scroll = 0;
            }
        }
        self.normalize_scroll(delta >= 0);
        if self.chapter != start_chapter {
            let _ = self.save_position();
        }
    }

    fn normalize_scroll(&mut self, down: bool) {
        if let Some((_, top, rows)) = self.view.item_at_row(self.scroll) {
            if rows > 1 && self.scroll > top {
                let target = if down { top + rows } else { top };
                if target >= self.view.total_rows
                    && down
                    && self.chapter + 1 < self.book.chapters.len()
                {
                    self.switch_chapter(self.chapter + 1);
                    return;
                }
                self.scroll = target;
            }
        }
        self.scroll = self.scroll.min(self.scroll_limit());
    }

    fn snap(&self, row: usize, down: bool) -> usize {
        if let Some((_, top, rows)) = self.view.item_at_row(row) {
            if rows > 1 && row > top {
                return if down { top + rows } else { top };
            }
        }
        row
    }

    fn max_scroll(&self) -> usize {
        self.view
            .total_rows
            .saturating_sub(usize::from(self.viewport_rows))
    }

    /// Highest valid top row for the current chapter.
    fn scroll_limit(&self) -> usize {
        if self.config.continuous && self.chapter + 1 < self.book.chapters.len()
        {
            self.view.total_rows.saturating_sub(1)
        } else {
            self.max_scroll()
        }
    }

    fn run_search(&mut self) {
        let query = self.input.trim().to_string();
        self.mode = Mode::Normal;
        if query.is_empty() {
            return;
        }

        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        'outer: for (chapter_index, chapter) in
            self.book.chapters.iter().enumerate()
        {
            for (block_index, text) in chapter.plain.iter().enumerate() {
                let haystack = text.to_lowercase();
                let mut start = 0;
                while let Some(position) = haystack[start..].find(&needle) {
                    let offset = start + position;
                    hits.push(Hit {
                        chapter: chapter_index,
                        block: block_index,
                        offset,
                    });
                    start = offset + needle.len().max(1);
                    if hits.len() >= MAX_HITS {
                        break 'outer;
                    }
                }
            }
        }

        if hits.is_empty() {
            self.search = None;
            self.rebuild_view();
            self.message = Some(format!("no matches for '{query}'"));
            return;
        }

        let current_block = self.view.block_at_row(self.scroll);
        let current = hits
            .iter()
            .position(|hit| {
                hit.chapter > self.chapter
                    || (hit.chapter == self.chapter
                        && hit.block > current_block)
            })
            .unwrap_or(0);

        self.search = Some(Search {
            query,
            hits,
            current,
        });
        self.rebuild_view();
        self.jump_to_current_hit();
    }

    fn jump_to_current_hit(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let hit = search.hits[search.current].clone();
        let query = search.query.clone();

        if hit.chapter != self.chapter {
            self.selected_link = None;
            self.chapter = hit.chapter;
            self.scroll = 0;
            self.rebuild_view();
        }
        if let Some(index) = self.view.find_hit(hit.block, &query, hit.offset) {
            let top = self.view.tops[index];
            let margin = usize::from(self.viewport_rows) / 3;
            let target = top.saturating_sub(margin);
            self.scroll = self.snap(target, true).min(self.scroll_limit());
        }
    }

    fn next_match(&mut self, forward: bool) {
        let Some(search) = self.search.as_mut() else {
            self.message = Some("no search".to_string());
            return;
        };
        if search.hits.is_empty() {
            return;
        }
        let length = search.hits.len();
        search.current = if forward {
            (search.current + 1) % length
        } else {
            (search.current + length - 1) % length
        };
        self.jump_to_current_hit();
    }

    fn open_theme_selector(&mut self) {
        self.theme_before =
            Some((self.config.theme.clone(), self.palette.clone()));
        self.theme_filter.clear();
        self.theme_matches = (0..theme::builtin_names().len()).collect();
        self.theme_selected = theme::builtin_names()
            .iter()
            .position(|(id, _)| *id == self.config.theme)
            .unwrap_or(0);
        self.theme_offset = 0;
        self.mode = Mode::Theme;
        if self.theme_matches.is_empty() {
            self.message = Some("no built-in themes".to_string());
        }
    }

    fn refresh_theme_matches(&mut self) {
        let needle = self.theme_filter.to_lowercase();
        self.theme_matches = theme::builtin_names()
            .iter()
            .enumerate()
            .filter(|(_, (id, name))| {
                needle.is_empty()
                    || id.to_lowercase().contains(&needle)
                    || name.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
        self.theme_selected = 0;
        self.preview_theme();
    }

    fn preview_theme(&mut self) {
        let Some(&index) = self.theme_matches.get(self.theme_selected) else {
            return;
        };
        let (id, _) = theme::builtin_names()[index];
        if id == self.config.theme {
            return;
        }
        match theme::load(id, &self.config.colors) {
            Ok((_theme, palette)) => {
                self.config.theme = id.to_string();
                self.palette = palette;
                self.rebuild_view();
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn select_theme(&mut self, delta: i64) {
        if self.theme_matches.is_empty() {
            return;
        }
        let last = self.theme_matches.len() - 1;
        let index =
            (self.theme_selected as i64 + delta).clamp(0, last as i64) as usize;
        if index != self.theme_selected {
            self.theme_selected = index;
            self.preview_theme();
        }
    }

    fn restore_theme(&mut self) {
        if let Some((name, palette)) = self.theme_before.take() {
            self.config.theme = name;
            self.palette = palette;
            self.rebuild_view();
        }
    }

    fn key_theme(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.restore_theme();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => self.apply_theme(),
            KeyCode::Down => self.select_theme(1),
            KeyCode::Up => self.select_theme(-1),
            KeyCode::PageDown => self.select_theme(10),
            KeyCode::PageUp => self.select_theme(-10),
            KeyCode::Home => {
                self.theme_selected = 0;
                self.preview_theme();
            }
            KeyCode::End => {
                self.theme_selected =
                    self.theme_matches.len().saturating_sub(1);
                self.preview_theme();
            }
            KeyCode::Backspace => {
                self.theme_filter.pop();
                self.refresh_theme_matches();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.theme_filter.push(ch);
                self.refresh_theme_matches();
            }
            _ => {}
        }
    }

    fn key_bookmark_label(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.bookmark_input.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let mut label = self.bookmark_input.trim().to_string();
                if label.is_empty() {
                    label = format!(
                        "{} {}%",
                        self.book.chapters[self.chapter].title,
                        self.book_percent()
                    );
                }
                let progress = if self.view.total_rows > 0 {
                    (self.scroll as f32 / self.view.total_rows as f32)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.bookmarks.push(Bookmark {
                    label: label.clone(),
                    chapter: self.chapter,
                    progress,
                    created: state::now(),
                });
                self.bookmark_input.clear();
                self.mode = Mode::Normal;
                let _ = self.save_position();
                self.message = Some(format!("bookmark added: {label}"));
            }
            KeyCode::Backspace => {
                self.bookmark_input.pop();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.bookmark_input.push(ch);
            }
            _ => {}
        }
    }

    fn key_bookmarks(&mut self, key: KeyEvent) {
        let last = self.bookmarks.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Up | KeyCode::Char('k') => {
                self.bookmark_selected =
                    self.bookmark_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.bookmark_selected = (self.bookmark_selected + 1).min(last)
            }
            KeyCode::PageUp => {
                self.bookmark_selected =
                    self.bookmark_selected.saturating_sub(10)
            }
            KeyCode::PageDown => {
                self.bookmark_selected = (self.bookmark_selected + 10).min(last)
            }
            KeyCode::Home | KeyCode::Char('g') => self.bookmark_selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.bookmark_selected = last,
            KeyCode::Enter => {
                let Some(bookmark) =
                    self.bookmarks.get(self.bookmark_selected).cloned()
                else {
                    return;
                };
                self.chapter =
                    bookmark.chapter.min(self.book.chapters.len() - 1);
                self.selected_link = None;
                self.rebuild_view();
                let row = (bookmark.progress * self.view.total_rows as f32)
                    .round() as usize;
                self.scroll = row.min(self.scroll_limit());
                self.mode = Mode::Normal;
                let _ = self.save_position();
            }
            KeyCode::Char('x') | KeyCode::Delete
                if self.bookmark_selected < self.bookmarks.len() =>
            {
                self.bookmarks.remove(self.bookmark_selected);
                self.bookmark_selected = self
                    .bookmark_selected
                    .min(self.bookmarks.len().saturating_sub(1));
                let _ = self.save_position();
                self.message = Some("bookmark deleted".to_string());
            }
            _ => {}
        }
    }

    /// Words left in the current chapter and in the whole book.
    fn words_left(&self) -> (usize, usize) {
        let local = if self.view.total_rows > 0 {
            (self.scroll as f64 / self.view.total_rows as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let chapter_words =
            self.chapter_words.get(self.chapter).copied().unwrap_or(0) as f64;
        let chapter_left = (chapter_words * (1.0 - local)).round() as usize;
        let book_left = chapter_left
            + self
                .chapter_words
                .iter()
                .skip(self.chapter + 1)
                .sum::<usize>();
        (chapter_left, book_left)
    }

    /// Seconds left in the current chapter and in the whole book.
    fn reading_seconds(&self) -> (u64, u64) {
        let (chapter_words, book_words) = self.words_left();
        let wpm = f64::from(self.config.reading_wpm).max(1.0);
        (
            (chapter_words as f64 * 60.0 / wpm).round() as u64,
            (book_words as f64 * 60.0 / wpm).round() as u64,
        )
    }

    /// Write the changed settings to the config file.
    fn save_settings(&mut self) {
        self.config.images = self.images;
        self.config.auto_scroll_wpm = self.auto_scroll_wpm;
        self.message = Some(match self.config.save_runtime() {
            Ok(path) => format!("settings saved to {}", path.display()),
            Err(error) => error.to_string(),
        });
    }

    /// Keep the previewed theme.
    fn apply_theme(&mut self) {
        self.theme_before = None;
        self.mode = Mode::Normal;
        let name = theme::builtin_names()
            .iter()
            .find(|(id, _)| *id == self.config.theme)
            .map(|(_, name)| *name)
            .unwrap_or(self.config.theme.as_str());
        self.message = Some(format!("theme: {name}"));
    }

    fn adjust_width(&mut self, delta: i32) {
        let max = self.content_width_limit();
        let width = (i32::from(self.config.text_width) + delta)
            .clamp(20, i32::from(max.max(20)));
        if width as u16 != self.config.text_width {
            self.config.text_width = width as u16;
            self.content_width = self.content_width_for(self.area);
            self.rebuild_view();
            self.message = Some(format!("width: {}", self.config.text_width));
        }
    }

    fn content_width_limit(&self) -> u16 {
        self.area
            .width
            .saturating_sub(self.config.margin.saturating_mul(2))
            .max(20)
    }

    fn content_width_for(&self, area: Rect) -> u16 {
        let available = area
            .width
            .saturating_sub(self.config.margin.saturating_mul(2));
        available
            .max(10)
            .min(self.config.text_width.max(10))
            .min(400)
            .min(area.width.max(1))
    }

    /// Inset the body by the configured top and bottom margins, keeping at
    /// least one row for text.
    fn inset_body(&self, area: Rect) -> Rect {
        let top = self.config.top_margin.min(area.height.saturating_sub(1));
        let bottom = self
            .config
            .bottom_margin
            .min(area.height.saturating_sub(top).saturating_sub(1));
        Rect {
            y: area.y.saturating_add(top),
            height: area.height.saturating_sub(top).saturating_sub(bottom),
            ..area
        }
    }

    fn ensure_size(&mut self, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content = self.content_width_for(area);
        let viewport = area.height.max(1);
        if !self.laid_out
            || content != self.content_width
            || viewport != self.viewport_rows
        {
            self.content_width = content;
            self.viewport_rows = viewport;
            self.area = area;
            self.rebuild_view();
            if !self.laid_out {
                self.laid_out = true;
                self.restore_progress();
            }
        }
    }

    fn restore_progress(&mut self) {
        if let Some(progress) = self.pending_progress.take() {
            let total = self.view.total_rows;
            let target = (f64::from(progress) * total as f64).round() as usize;
            let limit = self.scroll_limit();
            self.scroll = self.snap(target.min(limit), true).min(limit);
        }
    }

    fn build_view(&mut self, index: usize) -> View {
        let options = Options {
            width: self.content_width,
            paragraph_spacing: self.config.paragraph_spacing,
            image_max_rows: self
                .config
                .image_max_height
                .min(self.viewport_rows.saturating_sub(2).max(2)),
            images: self.images,
            trailing_blank_rows: if self.config.continuous
                && index + 1 < self.book.chapters.len()
            {
                2
            } else {
                0
            },
            query: self.search.as_ref().map(|search| search.query.clone()),
        };

        let book = &self.book;
        let chapter = &book.chapters[index];
        let palette = &self.palette;
        let picker = &self.picker;
        let dimensions = &mut self.image_dimensions;

        layout::build(chapter, palette, &options, picker, |src| {
            let key = book.image_key(&chapter.href, src)?;
            if let Some(size) = dimensions.get(&key) {
                return Some((key.clone(), size.0, size.1));
            }
            let bytes = book.image_bytes(&chapter.href, src)?;
            let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let (width, height) = reader.into_dimensions().ok()?;
            if dimensions.len() >= 512 {
                dimensions.clear();
            }
            dimensions.insert(key.clone(), (width, height));
            Some((key, width, height))
        })
    }

    fn rebuild_view(&mut self) {
        self.view = self.build_view(self.chapter);
        self.view_cache.clear();
        self.scroll = self.scroll.min(self.scroll_limit());
    }

    fn ensure_view(&mut self, index: usize) {
        if index == self.chapter || self.view_cache.contains_key(&index) {
            return;
        }
        let view = self.build_view(index);
        if self.view_cache.len() >= 8 {
            self.view_cache.clear();
        }
        self.view_cache.insert(index, view);
    }

    fn view_ref(&self, index: usize) -> &View {
        if index == self.chapter {
            &self.view
        } else {
            self.view_cache.get(&index).expect("chapter view is built")
        }
    }

    fn switch_chapter(&mut self, index: usize) {
        if index == self.chapter || index >= self.book.chapters.len() {
            return;
        }
        self.selected_link = None;
        self.chapter = index;
        self.view = self.build_view(index);
        self.view_cache.clear();
        self.scroll = 0;
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        match self.mode {
            Mode::Search => {
                let line = bar_line(
                    &format!("/ {}", self.input),
                    "",
                    area.width,
                    self.palette.status(),
                );
                frame.render_widget(Paragraph::new(line), area);
                return;
            }
            Mode::Percent => {
                let line = bar_line(
                    &format!("goto {}%", self.input),
                    "",
                    area.width,
                    self.palette.status(),
                );
                frame.render_widget(Paragraph::new(line), area);
                return;
            }
            Mode::BookmarkLabel => {
                let line = bar_line(
                    &format!("bookmark: {}", self.bookmark_input),
                    "",
                    area.width,
                    self.palette.status(),
                );
                frame.render_widget(Paragraph::new(line), area);
                return;
            }
            _ => {}
        }

        let progress = self.book_percent();
        let auto = if self.auto_scroll {
            format!("auto {}wpm  ", self.auto_scroll_wpm)
        } else {
            String::new()
        };
        let (_, book_seconds) = self.reading_seconds();
        let right = format!(
            "{auto}~{}  {}/{}  {progress}%  ? help ",
            format_duration(book_seconds),
            self.chapter + 1,
            self.book.chapters.len()
        );
        let width = usize::from(area.width);
        let left_limit = width
            .saturating_sub(layout::display_width(&right).saturating_add(1));
        let mut left = if let Some(message) = &self.message {
            format!(" {message}")
        } else {
            title_line(
                &self.book.title,
                self.book.author.as_deref(),
                left_limit,
            )
        };
        if let Some(search) = &self.search {
            left.push_str(&format!(
                "  [{}/{}]",
                search.current + 1,
                search.hits.len()
            ));
        }
        if !self.images {
            left.push_str("  [images off]");
        }
        if self.config.continuous {
            left.push_str("  [continuous]");
        }
        let line = bar_line(&left, &right, area.width, self.palette.status());
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_body(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Block::new().style(self.palette.base()), area);
        if self.book.chapters.is_empty() {
            return;
        }

        enum Paint {
            Line(Line<'static>),
            Image { key: String, alt: String, cols: u16 },
        }

        let width = usize::from(area.width);
        let content_width = usize::from(self.content_width).min(width).max(1);
        let x = area.x + ((width - content_width) / 2) as u16;
        // Keep a blank column between the indicator and the first glyph.
        let bar_x = x
            .checked_sub(2)
            .filter(|bar| *bar >= area.x)
            .or_else(|| x.checked_sub(1).filter(|bar| *bar >= area.x));

        let mut chapter = self.chapter;
        let mut row = self.scroll;
        let mut y = area.y;

        while y < area.bottom() && chapter < self.book.chapters.len() {
            self.ensure_view(chapter);

            let (mut index, total_rows, item_count) = {
                let view = self.view_ref(chapter);
                (
                    view.item_at_row(row)
                        .map(|(index, _, _)| index)
                        .unwrap_or(0),
                    view.total_rows,
                    view.items.len(),
                )
            };
            if total_rows == 0 || row >= total_rows {
                if !self.config.continuous {
                    break;
                }
                chapter += 1;
                row = 0;
                continue;
            }

            while index < item_count && y < area.bottom() {
                let (paint, rows, top, marker) = {
                    let view = self.view_ref(chapter);
                    let rows =
                        u16::try_from(view.rows(index)).unwrap_or(u16::MAX);
                    let top = view.tops[index];
                    let marker = match &view.items[index] {
                        layout::Item::Line { plain, .. }
                            if plain.is_empty() =>
                        {
                            None
                        }
                        _ => view.paragraph_of_item(index).map(|ordinal| {
                            self.chapter_paragraph_offsets
                                .get(chapter)
                                .copied()
                                .unwrap_or(0)
                                + ordinal
                        }),
                    };
                    let paint = match &view.items[index] {
                        layout::Item::Line { line, .. } => {
                            Paint::Line(line.clone())
                        }
                        layout::Item::Image { key, alt, cols, .. } => {
                            Paint::Image {
                                key: key.clone(),
                                alt: alt.clone(),
                                cols: *cols,
                            }
                        }
                    };
                    (paint, rows, top, marker)
                };

                if top < row {
                    index += 1;
                    continue;
                }

                let height = rows.min(area.bottom() - y);

                if self.config.paragraph_colors {
                    if let (Some(bar_x), Some(marker)) = (bar_x, marker) {
                        let color = self.palette.markers
                            [marker % self.palette.markers.len()];
                        let style = Style::default()
                            .fg(color)
                            .bg(self.palette.background);
                        for offset in 0..height {
                            let rect = Rect::new(bar_x, y + offset, 1, 1);
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(
                                    "\u{2502}", style,
                                ))),
                                rect,
                            );
                        }
                    }
                }

                match paint {
                    Paint::Line(line) => {
                        let rect = Rect::new(x, y, content_width as u16, 1);
                        frame.render_widget(Paragraph::new(line), rect);
                        y += 1;
                    }
                    Paint::Image { key, alt, cols } => {
                        let image_width = cols.min(content_width as u16).max(1);
                        let image_x =
                            x + ((content_width as u16 - image_width) / 2);
                        let rect = Rect::new(image_x, y, image_width, height);
                        if let Some(protocol) =
                            self.protocol_for(&key, image_width, rows)
                        {
                            frame.render_widget(
                                Image::new(protocol).allow_clipping(true),
                                rect,
                            );
                        } else {
                            let label = if alt.is_empty() {
                                "[image]".to_string()
                            } else {
                                format!("[image: {alt}]")
                            };
                            frame.render_widget(
                                Paragraph::new(label)
                                    .style(self.palette.muted()),
                                rect,
                            );
                        }
                        y = y.saturating_add(height);
                        if height < rows {
                            y = area.bottom();
                        }
                    }
                }

                index += 1;
            }

            if y >= area.bottom() || !self.config.continuous {
                break;
            }
            chapter += 1;
            row = 0;
        }
    }

    fn protocol_for(
        &mut self,
        key: &str,
        cols: u16,
        rows: u16,
    ) -> Option<&Protocol> {
        let cache_key = (key.to_string(), cols.max(1), rows.max(1));
        if !self.image_protocols.contains_key(&cache_key) {
            let bytes = self.book.image_bytes_by_key(key)?;
            let image = image::load_from_memory(&bytes).ok()?;
            let protocol = self
                .picker
                .new_protocol(
                    image,
                    Size::new(cols.max(1), rows.max(1)),
                    Resize::Fit(None),
                )
                .ok()?;
            if self.image_protocols.len() >= 24 {
                self.image_protocols.clear();
            }
            self.image_protocols.insert(cache_key.clone(), protocol);
        }
        self.image_protocols.get(&cache_key)
    }

    fn draw_toc(&mut self, frame: &mut Frame, area: Rect) {
        self.toc_list_area = None;
        if area.width < 12 || area.height < 5 {
            return;
        }
        let width = 72.min(area.width.saturating_sub(4)).max(20);
        let total = self.book.toc.len();
        if total == 0 {
            let height = 5;
            let rect = centered(area, width, height);
            frame.render_widget(Clear, rect);
            let block = Block::bordered()
                .title(" Contents ")
                .border_style(self.palette.border_style())
                .style(self.palette.base());
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            let lines = vec![
                Line::styled(
                    " No table of contents in this book.",
                    self.palette.muted(),
                ),
                Line::styled(
                    " Use h/l, %, or / to move.",
                    self.palette.muted(),
                ),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let max_list = usize::from(area.height.saturating_sub(6).max(1));
        let visible_entries = self.toc_matches.len().min(max_list);
        let height = (visible_entries as u16 + 4)
            .min(area.height.saturating_sub(2))
            .max(6);
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);

        let block = Block::bordered()
            .title(" Contents ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let [search_area, list_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

        let label = "filter: ";
        let mut search = String::from(label);
        search.push_str(&self.toc_filter);
        frame.render_widget(
            Paragraph::new(Line::styled(search, self.palette.muted())),
            search_area,
        );
        let cursor_x = search_area
            .x
            .saturating_add(layout::display_width(label) as u16)
            .saturating_add(layout::display_width(&self.toc_filter) as u16);
        if cursor_x < search_area.right() {
            frame.set_cursor_position(Position::new(cursor_x, search_area.y));
        }

        let rows = usize::from(list_area.height);
        if self.toc_selected < self.toc_offset {
            self.toc_offset = self.toc_selected;
        } else if rows > 0 && self.toc_selected >= self.toc_offset + rows {
            self.toc_offset = self.toc_selected + 1 - rows;
        }

        let mut lines = Vec::new();
        if self.toc_matches.is_empty() {
            lines.push(Line::styled(
                " no matching entries",
                self.palette.muted(),
            ));
        } else {
            for (index, entry_index) in self
                .toc_matches
                .iter()
                .enumerate()
                .skip(self.toc_offset)
                .take(rows)
            {
                let entry = &self.book.toc[*entry_index];
                let style = if index == self.toc_selected {
                    self.palette.selected()
                } else {
                    self.palette.base()
                };
                let marker = if entry.chapter == self.chapter {
                    "\u{25b8}"
                } else {
                    " "
                };
                let indent = "  ".repeat(entry.depth.min(4));
                lines.push(Line::from(vec![Span::styled(
                    format!("{marker} {indent}{}", entry.title),
                    style,
                )]));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);
        if !self.toc_matches.is_empty() {
            self.toc_list_area = Some(list_area);
        }

        frame.render_widget(
            Paragraph::new(Line::styled(
                " enter open   esc close   up/down select ",
                self.palette.muted(),
            )),
            hint_area,
        );
    }

    fn draw_theme_selector(&mut self, frame: &mut Frame, area: Rect) {
        self.theme_list_area = None;
        if area.width < 24 || area.height < 6 {
            return;
        }
        let width = 64.min(area.width.saturating_sub(4)).max(24);
        let max_list = usize::from(area.height.saturating_sub(6).max(1));
        let visible = self.theme_matches.len().min(max_list);
        let height = (visible as u16 + 4)
            .min(area.height.saturating_sub(2))
            .max(6);
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);

        let block = Block::bordered()
            .title(" Themes ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let [search_area, list_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

        let label = "filter: ";
        let mut search = String::from(label);
        search.push_str(&self.theme_filter);
        frame.render_widget(
            Paragraph::new(Line::styled(search, self.palette.muted())),
            search_area,
        );
        let cursor_x = search_area
            .x
            .saturating_add(layout::display_width(label) as u16)
            .saturating_add(layout::display_width(&self.theme_filter) as u16);
        if cursor_x < search_area.right() {
            frame.set_cursor_position(Position::new(cursor_x, search_area.y));
        }

        let rows = usize::from(list_area.height);
        if self.theme_selected < self.theme_offset {
            self.theme_offset = self.theme_selected;
        } else if rows > 0 && self.theme_selected >= self.theme_offset + rows {
            self.theme_offset = self.theme_selected + 1 - rows;
        }

        let names = theme::builtin_names();
        let mut lines = Vec::new();
        if self.theme_matches.is_empty() {
            lines.push(Line::styled(
                " no matching themes",
                self.palette.muted(),
            ));
        } else {
            for index in
                self.theme_offset..self.theme_offset.saturating_add(rows)
            {
                let Some(&name_index) = self.theme_matches.get(index) else {
                    break;
                };
                let (id, name) = names[name_index];
                let selected = index == self.theme_selected;
                let style = if selected {
                    self.palette.selected()
                } else {
                    self.palette.base()
                };
                let marker = if selected { "\u{25b8}" } else { " " };
                lines.push(Line::styled(
                    format!("{marker} {name}  ({id})"),
                    style,
                ));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);
        if !self.theme_matches.is_empty() {
            self.theme_list_area = Some(list_area);
        }

        frame.render_widget(
            Paragraph::new(Line::styled(
                " enter apply   esc cancel   up/down select ",
                self.palette.muted(),
            )),
            hint_area,
        );
    }

    fn draw_bookmarks(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < 12 || area.height < 5 {
            return;
        }
        let width = 72.min(area.width.saturating_sub(4)).max(20);
        let height = (self.bookmarks.len() as u16 + 2)
            .clamp(5, area.height.saturating_sub(2).max(5));
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);

        let block = Block::bordered()
            .title(" Bookmarks ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let [list_area, hint_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(inner);

        let rows = usize::from(list_area.height);
        if self.bookmark_selected < self.bookmark_offset {
            self.bookmark_offset = self.bookmark_selected;
        } else if rows > 0
            && self.bookmark_selected >= self.bookmark_offset + rows
        {
            self.bookmark_offset = self.bookmark_selected + 1 - rows;
        }

        let mut lines = Vec::new();
        if self.bookmarks.is_empty() {
            lines.push(Line::styled(" no bookmarks", self.palette.muted()));
        } else {
            let chapter_count = self.book.chapters.len();
            for index in
                self.bookmark_offset..self.bookmark_offset.saturating_add(rows)
            {
                let Some(bookmark) = self.bookmarks.get(index) else {
                    break;
                };
                let style = if index == self.bookmark_selected {
                    self.palette.selected()
                } else {
                    self.palette.base()
                };
                let title = self
                    .book
                    .chapters
                    .get(bookmark.chapter)
                    .map(|chapter| chapter.title.as_str())
                    .unwrap_or_default();
                let percent =
                    (bookmark.progress * 100.0).round().clamp(0.0, 100.0);
                lines.push(Line::styled(
                    format!(
                        " {}  {}/{} {title}  {percent}%",
                        bookmark.label,
                        bookmark.chapter + 1,
                        chapter_count
                    ),
                    style,
                ));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(Line::styled(
                " enter jump   x delete   esc close ",
                self.palette.muted(),
            )),
            hint_area,
        );
    }

    fn draw_stats(&self, frame: &mut Frame, area: Rect) {
        if area.width < 20 || area.height < 6 {
            return;
        }
        let width = 60.min(area.width.saturating_sub(4)).max(24);
        // Seven lines plus the two border rows.
        let height = 9.min(area.height.saturating_sub(2)).max(6);
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);

        let block = Block::bordered()
            .title(" Reading stats ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let (chapter_words, book_words) = self.words_left();
        let (chapter_seconds, book_seconds) = self.reading_seconds();
        let title = match self.book.author.as_deref() {
            Some(author) => format!(" {} \u{2014} {author}", self.book.title),
            None => format!(" {}", self.book.title),
        };
        let chapter_title = self
            .book
            .chapters
            .get(self.chapter)
            .map(|chapter| chapter.title.as_str())
            .unwrap_or_default();

        let lines = vec![
            Line::styled(title, self.palette.accent()),
            Line::styled(
                format!(
                    " Chapter {}/{}: {chapter_title}",
                    self.chapter + 1,
                    self.book.chapters.len()
                ),
                self.palette.base(),
            ),
            Line::styled(
                format!(" Book progress: {}%", self.book_percent()),
                self.palette.base(),
            ),
            Line::styled(
                format!(
                    " Words left: {chapter_words} chapter, {book_words} book"
                ),
                self.palette.base(),
            ),
            Line::styled(
                format!(
                    " Time left: {} chapter, {} book",
                    format_duration(chapter_seconds),
                    format_duration(book_seconds)
                ),
                self.palette.base(),
            ),
            Line::styled(
                format!(" Speed: {} wpm", self.config.reading_wpm),
                self.palette.muted(),
            ),
            Line::styled(" esc close", self.palette.muted()),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        const HELP: &[&str] = &[
            " j / k        line down / up",
            " d / u        half page down / up",
            " Space        next paragraph",
            " PgDn / PgUp  page down / up",
            " a            auto-scroll; [ / ] speed",
            " g / G        start / end of chapter",
            " h / l        previous / next chapter",
            " Tab          next link",
            " Enter        follow or open",
            " Ctrl+O       back after link",
            " t            contents (fuzzy search)",
            " %            jump to a percentage",
            " /            full-text search",
            " n / N        next / previous match",
            " m            add bookmark",
            " '            bookmarks",
            " i            toggle images",
            " c            toggle continuous mode",
            " p            toggle paragraph colors",
            " r            pull latest sync position",
            " Ctrl+T       theme selector",
            " + / -        wider / narrower text",
            " s / S        reading time / save settings",
            " mouse        wheel, click popups",
            " ?            this help",
            " q            quit",
        ];
        if area.width < 20 || area.height < 8 {
            return;
        }
        let width = 46.min(area.width.saturating_sub(4)).max(20);
        let height = (HELP.len() as u16 + 2).min(area.height.saturating_sub(2));
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .title(" Keys ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let lines: Vec<Line> = HELP
            .iter()
            .map(|line| Line::styled((*line).to_string(), self.palette.base()))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.save_position();
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Format the book title and author within a width, keeping the author when
/// the title does not fit.
fn title_line(title: &str, author: Option<&str>, limit: usize) -> String {
    match author {
        None => {
            let title = truncate(title, limit.saturating_sub(1));
            format!(" {title}")
        }
        Some(author) => {
            let suffix = format!(" \u{2014} {author}");
            let suffix_width = layout::display_width(&suffix);
            if limit <= suffix_width {
                return truncate(&suffix, limit);
            }
            let title = truncate(title, limit - suffix_width - 1);
            format!(" {title}{suffix}")
        }
    }
}

fn bar_line(
    left: &str,
    right: &str,
    width: u16,
    style: Style,
) -> Line<'static> {
    let width = usize::from(width);
    let right = truncate(right, width);
    let right_width = layout::display_width(&right);
    let left_limit = width.saturating_sub(right_width.saturating_add(1));
    let left = truncate(left, left_limit);
    let used = layout::display_width(&left) + right_width;
    let padding = width.saturating_sub(used).max(1);
    Line::from(vec![Span::styled(
        format!("{left}{}{right}", " ".repeat(padding)),
        style,
    )])
}

/// Format a duration in seconds as a short string.
fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = (seconds + 30) / 60;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {}m", minutes / 60, minutes % 60)
    }
}

fn truncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let char_width = layout::character_width(ch);
        if used + char_width > width {
            break;
        }
        used += char_width;
        out.push(ch);
    }
    out
}

/// Whole-book fraction for a chapter and a fraction inside it.
fn book_fraction_at(
    chapter_chars: &[usize],
    total_chars: usize,
    chapter: usize,
    progress: f32,
) -> f32 {
    if total_chars == 0 {
        return 0.0;
    }
    let before: usize = chapter_chars.iter().take(chapter).sum();
    let chars = chapter_chars.get(chapter).copied().unwrap_or(0);
    ((before as f32 + progress.clamp(0.0, 1.0) * chars as f32)
        / total_chars as f32)
        .clamp(0.0, 1.0)
}

/// Footer message for a remote position.
fn remote_label(remote: &RemoteProgress) -> String {
    let percent = (remote.percentage * 100.0).round() as u32;
    let device = if remote.device.trim().is_empty() {
        "another device"
    } else {
        remote.device.trim()
    };
    format!("sync: {percent}% from {device}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::io::{BufRead, Read, Write};
    use std::sync::mpsc;
    use std::thread;

    fn app(
        fixture: &crate::testutil::TestBook,
    ) -> (App, PathBuf, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("positions.toml");
        let app = App::new(
            &fixture.path,
            Config::default(),
            Picker::halfblocks(),
            state_path.clone(),
        )
        .unwrap();
        (app, state_path, directory)
    }

    fn screen(app: &mut App) -> String {
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        assert!(!app.handle_key(key).unwrap());
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            press(app, KeyCode::Char(ch));
        }
    }

    fn press_ctrl(app: &mut App, code: KeyCode) {
        let key = KeyEvent::new(code, KeyModifiers::CONTROL);
        assert!(!app.handle_key(key).unwrap());
    }

    #[test]
    fn draws_first_chapter() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let text = screen(&mut app);
        assert!(text.contains("Test Book"));
        assert!(text.contains("Alice was beginning"));
    }

    struct Captured {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    fn serve_requests(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, mpsc::Receiver<Captured>, thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        let name = name.trim().to_ascii_lowercase();
                        if name == "content-length" {
                            length = value.trim().parse().unwrap_or(0);
                        }
                        headers.push((name, value.trim().to_string()));
                    }
                }
                let mut request_body = vec![0u8; length];
                reader.read_exact(&mut request_body).unwrap();
                sender
                    .send(Captured {
                        method,
                        path,
                        headers,
                        body: String::from_utf8(request_body).unwrap(),
                    })
                    .unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), receiver, handle)
    }

    fn sync_config(server: String) -> Config {
        Config {
            sync: crate::config::SyncConfig {
                enabled: true,
                server,
                username: "reader".to_string(),
                password: "secret".to_string(),
                ..Default::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn adopts_newer_remote_progress() {
        let (server, _requests, handle) = serve_requests(vec![(
            "200 OK",
            r#"{"percentage":0.5,"timestamp":100,"device":"CrossPoint"}"#,
        )]);
        let directory = tempfile::tempdir().unwrap();
        let app = App::new(
            Path::new("tests/sample.epub"),
            sync_config(server),
            Picker::halfblocks(),
            directory.path().join("state.toml"),
        )
        .unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some("sync: 50% from CrossPoint")
        );
        assert!(app.pending_progress.is_some());
        assert!(app.chapter() > 0);
        handle.join().unwrap();
    }

    #[test]
    fn r_fetches_the_latest_remote_position() {
        let (server, requests, handle) = serve_requests(vec![
            (
                "200 OK",
                r#"{"percentage":0.1,"timestamp":100,"device":"A"}"#,
            ),
            (
                "200 OK",
                r#"{"percentage":0.8,"timestamp":200,"device":"B"}"#,
            ),
        ]);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            Path::new("tests/sample.epub"),
            sync_config(server),
            Picker::halfblocks(),
            directory.path().join("state.toml"),
        )
        .unwrap();
        screen(&mut app);
        assert_eq!(requests.recv().unwrap().method, "GET");
        let before = app.book_percent();

        press(&mut app, KeyCode::Char('r'));
        assert!(screen(&mut app).contains("sync: 80% from B"));
        assert!(app.book_percent() >= 70, "was {}", app.book_percent());
        assert!(app.book_percent() > before);
        assert_eq!(requests.recv().unwrap().method, "GET");
        handle.join().unwrap();
    }

    #[test]
    fn r_reports_disabled_sync() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('r'));
        assert!(screen(&mut app).contains("sync is not enabled"));
    }

    #[test]
    fn keeps_newer_local_progress() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.toml");
        let book = Path::new("tests/sample.epub");
        let key = fs::canonicalize(book)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut store = Store::default();
        store.books.push(BookPosition {
            path: key,
            chapter: 6,
            progress: 0.9,
            updated: 9_999_999_999,
            chapters: 7,
            book_progress: 0.9,
            ..Default::default()
        });
        store.save(&state_path).unwrap();

        let (server, _requests, handle) = serve_requests(vec![(
            "200 OK",
            r#"{"percentage":0.1,"timestamp":100,"device":"Other"}"#,
        )]);
        let app = App::new(
            book,
            sync_config(server),
            Picker::halfblocks(),
            state_path,
        )
        .unwrap();
        assert_eq!(app.chapter(), 6);
        assert!(
            (app.pending_progress.unwrap() - 0.9).abs() < 0.0001,
            "progress was {:?}",
            app.pending_progress
        );
        assert_eq!(app.message, None);
        handle.join().unwrap();
    }

    #[test]
    fn pushes_after_a_successful_pull() {
        let (server, requests, handle) = serve_requests(vec![
            ("200 OK", "{}"),
            ("200 OK", r#"{"document":"x","timestamp":1}"#),
        ]);
        let directory = tempfile::tempdir().unwrap();
        let app = App::new(
            Path::new("tests/sample.epub"),
            sync_config(server),
            Picker::halfblocks(),
            directory.path().join("state.toml"),
        )
        .unwrap();
        app.push_sync().unwrap();

        let pull = requests.recv().unwrap();
        assert_eq!(pull.method, "GET");
        let push = requests.recv().unwrap();
        assert_eq!(push.method, "PUT");
        assert_eq!(push.path, "/syncs/progress");
        assert!(push.headers.contains(&(
            "x-auth-key".to_string(),
            "5ebe2294ecd0e0f08eab7690d2a6ee69".to_string()
        )));
        let body: serde_json::Value =
            serde_json::from_str(&push.body).unwrap();
        assert_eq!(body["document"], "2babf939094150df4dd24c53baf2b89a");
        assert_eq!(body["device"], "slurp");
        handle.join().unwrap();
    }

    #[test]
    fn maps_book_fractions_to_chapters() {
        let fixture = crate::testutil::sample_book();
        let (app, _, _directory) = app(&fixture);
        for chapter in 0..app.chapter_chars.len() {
            let chars = app.chapter_chars[chapter] as f32;
            if chars == 0.0 {
                continue;
            }
            for progress in [0.0, 0.25, 0.75] {
                let fraction = book_fraction_at(
                    &app.chapter_chars,
                    app.total_chars,
                    chapter,
                    progress,
                );
                let (found, local) = app.locate_fraction(fraction);
                assert_eq!(found, chapter);
                assert!(
                    (local - progress).abs() <= 1.0 / chars + 0.001,
                    "chapter {chapter}: {local} != {progress}"
                );
            }
        }
        let (last, local) = app.locate_fraction(1.0);
        assert_eq!(last, app.chapter_chars.len() - 1);
        assert!((local - 1.0).abs() < 0.001);
    }

    #[test]
    fn moves_between_chapters_and_remembers_position() {
        let fixture = crate::testutil::sample_book();
        let (mut app, state_path, _directory) = app(&fixture);

        screen(&mut app);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.chapter(), 2);
        screen(&mut app);
        press(&mut app, KeyCode::Char('G'));
        let saved_scroll = app.scroll();
        assert!(saved_scroll > 0);
        app.save_position().unwrap();
        drop(app);

        let restored = App::new(
            &fixture.path,
            Config::default(),
            Picker::halfblocks(),
            state_path,
        )
        .unwrap();
        assert_eq!(restored.chapter(), 2);

        let mut restored = restored;
        screen(&mut restored);
        assert_eq!(restored.chapter(), 2);
        assert_eq!(restored.scroll(), saved_scroll);
    }

    #[test]
    fn searches_all_chapters() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);

        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.mode(), Mode::Search);
        type_text(&mut app, "theta");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.mode(), Mode::Normal);
        let search = app.search().expect("search results");
        assert!(search.hits.len() > 1);
        assert_eq!(app.chapter(), 2);

        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.search().unwrap().current, 1);
    }

    #[test]
    fn jumps_to_book_percentage() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);

        press(&mut app, KeyCode::Char('%'));
        assert_eq!(app.mode(), Mode::Percent);
        type_text(&mut app, "50");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.mode(), Mode::Normal);
        let middle = app.book_percent();
        assert!((45..=55).contains(&middle), "middle was {middle}");
        assert!(app.chapter() > 0);

        press(&mut app, KeyCode::Char('%'));
        type_text(&mut app, "100");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.book_percent(), 100);

        press(&mut app, KeyCode::Char('%'));
        type_text(&mut app, "0");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.chapter(), 0);
        assert_eq!(app.scroll(), 0);
        assert_eq!(app.book_percent(), 0);
    }

    #[test]
    fn continuous_mode_scrolls_across_chapters() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);

        press(&mut app, KeyCode::Char('c'));
        let last = app.book.chapters.len() - 1;
        app.scroll_by(10_000);
        assert_eq!(app.chapter(), last);

        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.chapter(), 0);
        assert_eq!(app.scroll(), 0);

        app.scroll_by(10_000);
        assert_eq!(app.chapter(), last);
        app.scroll_by(-10_000);
        assert_eq!(app.chapter(), 0);
        assert_eq!(app.scroll(), 0);
    }

    #[test]
    fn auto_scroll_crosses_chapters_in_continuous_mode() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('a'));

        app.advance_auto_scroll(Duration::from_secs(60));

        assert!(app.chapter() > 0);
    }

    #[test]
    fn continuous_mode_adds_two_blank_rows_between_chapters() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        let before = app.view().total_rows;

        press(&mut app, KeyCode::Char('c'));

        assert_eq!(app.view().total_rows, before + 2);
        let tail: Vec<_> = app.view().items.iter().rev().take(2).collect();
        for item in tail {
            assert!(
                matches!(item, layout::Item::Line { plain, .. } if plain.is_empty())
            );
        }
    }

    #[test]
    fn continuous_mode_draws_the_next_chapter() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('c'));

        let rows = app.view().total_rows;
        app.scroll = rows.saturating_sub(2);

        let text = screen(&mut app);
        assert_eq!(app.chapter(), 0);
        assert!(text.contains("Part Two"));
    }

    #[test]
    fn normal_mode_stops_at_the_chapter_end() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);

        app.scroll_by(10_000);

        assert_eq!(app.chapter(), 0);
        assert_eq!(app.scroll(), app.max_scroll());
    }

    #[test]
    fn auto_scroll_advances_the_view() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app);
        press(&mut app, KeyCode::Char('a'));
        let before = app.scroll();

        app.advance_auto_scroll(Duration::from_secs(60));

        assert!(app.scroll() > before);
        assert!(screen(&mut app).contains("auto 200wpm"));
    }

    #[test]
    fn auto_scroll_pauses_in_popups() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app);
        press(&mut app, KeyCode::Char('a'));

        let before = app.scroll();
        press(&mut app, KeyCode::Char('t'));
        app.advance_auto_scroll(Duration::from_secs(60));
        assert_eq!(app.scroll(), before);
        press(&mut app, KeyCode::Esc);
    }

    #[test]
    fn auto_scroll_moves_to_the_next_chapter() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.chapter(), 0);

        app.advance_auto_scroll(Duration::from_secs(30));

        assert_eq!(app.chapter(), 1);
    }

    #[test]
    fn auto_scroll_stops_at_the_end_of_the_book() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('%'));
        type_text(&mut app, "100");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('a'));

        app.advance_auto_scroll(Duration::from_secs(120));

        assert!(screen(&mut app).contains("auto-scroll: end of book"));
    }

    #[test]
    fn changes_auto_scroll_speed() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        assert!(screen(&mut app).contains("auto 250wpm"));
    }

    #[test]
    fn toggles_images() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('l'));
        assert!(app
            .view()
            .items
            .iter()
            .any(|item| matches!(item, layout::Item::Image { .. })));

        press(&mut app, KeyCode::Char('i'));
        assert!(!app
            .view()
            .items
            .iter()
            .any(|item| matches!(item, layout::Item::Image { .. })));
        let text = screen(&mut app);
        assert!(text.contains("images off"));
    }

    #[test]
    fn vertical_margins_are_blank() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        };
        assert!(row(0).trim().is_empty());
        assert!(!row(1).trim().is_empty());
        assert!(row(28).trim().is_empty());
    }

    #[test]
    fn custom_vertical_margins_shift_the_text() {
        let fixture = crate::testutil::sample_book();
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            top_margin: 3,
            bottom_margin: 2,
            ..Config::default()
        };
        let mut app = App::new(
            &fixture.path,
            config,
            Picker::halfblocks(),
            directory.path().join("positions.toml"),
        )
        .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        };
        for blank in 0..3 {
            assert!(row(blank).trim().is_empty(), "row {blank} is not blank");
        }
        assert!(!row(3).trim().is_empty());
        for blank in 27..=28 {
            assert!(row(blank).trim().is_empty(), "row {blank} is not blank");
        }
    }

    #[test]
    fn paragraph_colors_draw_a_cycling_gutter() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);

        let mut terminal =
            Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let body_rows =
            terminal.backend().buffer().area.height.saturating_sub(1);
        let buffer = terminal.backend().buffer();
        assert!((0..body_rows).all(|y| {
            buffer[(0, y)].symbol() != "\u{2502}"
        }));

        press(&mut app, KeyCode::Char('p'));
        assert!(app.config.paragraph_colors);

        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut runs = Vec::new();
        let mut previous = None;
        for y in 0..body_rows {
            let cell = &buffer[(0, y)];
            if cell.symbol() != "\u{2502}" {
                continue;
            }
            let color = cell.fg;
            if previous != Some(color) {
                runs.push(color);
            }
            previous = Some(color);
        }
        assert!(runs.len() >= 2, "expected several paragraph bars");
        for pair in runs.windows(2) {
            let index = |color| {
                app.palette
                    .markers
                    .iter()
                    .position(|marker| *marker == color)
                    .unwrap_or_else(|| {
                        panic!("bar color {color:?} is not a marker")
                    })
            };
            assert_eq!(index(pair[1]), (index(pair[0]) + 1) % 5);
        }
        assert!(screen(&mut app).contains("paragraph colors: on"));
    }

    #[test]
    fn paragraph_colors_continue_across_chapters() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('c'));
        let last_text = app
            .view
            .paragraph_starts
            .iter()
            .rev()
            .nth(1)
            .copied()
            .expect("chapter one has text paragraphs");
        app.scroll = app.view.tops[last_text];
        assert_eq!(
            app.chapter_paragraph_offsets[1],
            app.view.paragraph_starts.len()
        );

        let mut terminal =
            Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut runs = Vec::new();
        let mut previous = None;
        for y in 0..buffer.area.height.saturating_sub(1) {
            let cell = &buffer[(0, y)];
            if cell.symbol() != "\u{2502}" {
                continue;
            }
            let color = cell.fg;
            if previous != Some(color) {
                runs.push(color);
            }
            previous = Some(color);
        }
        assert!(runs.len() >= 5, "expected paragraphs from both chapters");
        for pair in runs.windows(2) {
            assert_ne!(
                pair[0], pair[1],
                "neighboring paragraphs share a color"
            );
            let index = |color| {
                app.palette
                    .markers
                    .iter()
                    .position(|marker| *marker == color)
                    .expect("bar color is a marker color")
            };
            assert_eq!(index(pair[1]), (index(pair[0]) + 1) % 5);
        }
    }

    #[test]
    fn space_moves_to_the_next_paragraph() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app);
        assert_eq!(app.scroll(), 0);

        press(&mut app, KeyCode::Char(' '));
        let second = app.scroll();
        assert!(second > 0);
        assert!(
            app.view()
                .paragraph_starts
                .iter()
                .any(|index| app.view().tops[*index] == second),
            "space must stop at a paragraph start"
        );

        press(&mut app, KeyCode::Char(' '));
        assert!(app.scroll() > second);
    }

    #[test]
    fn follows_and_returns_from_links() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        assert_eq!(app.chapter(), 0);
        assert!(!app.book.chapters[0].links.is_empty());

        press(&mut app, KeyCode::Tab);
        assert!(screen(&mut app).contains("link 1/1"));
        let at_link = app.scroll();

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.chapter(), 2);

        press_ctrl(&mut app, KeyCode::Char('o'));
        assert_eq!(app.chapter(), 0);
        assert_eq!(app.scroll(), at_link);
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn reports_missing_and_external_links() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        app.book.chapters[0].links.push(crate::html::Link {
            href: "https://example.com".into(),
            label: "web".into(),
            block: 0,
        });
        app.book.chapters[0].links.push(crate::html::Link {
            href: "gone.xhtml".into(),
            label: "gone".into(),
            block: 0,
        });

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(screen(&mut app).contains("external link"));

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(screen(&mut app).contains("link target not found"));
        assert_eq!(app.chapter(), 0);
    }

    #[test]
    fn adds_jumps_and_persists_bookmarks() {
        let fixture = crate::testutil::sample_book();
        let (mut app, state_path, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app);
        press(&mut app, KeyCode::Char('d'));
        let chapter = app.chapter();
        let scroll = app.scroll();
        assert!(scroll > 0);

        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode(), Mode::BookmarkLabel);
        assert!(screen(&mut app).contains("bookmark: "));
        type_text(&mut app, "note");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.bookmarks.len(), 1);
        assert!(screen(&mut app).contains("bookmark added: note"));

        press(&mut app, KeyCode::Char('h'));
        assert_ne!(app.chapter(), chapter);

        press(&mut app, KeyCode::Char('\''));
        assert_eq!(app.mode(), Mode::Bookmarks);
        assert!(screen(&mut app).contains("note"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.chapter(), chapter);
        assert_eq!(app.scroll(), scroll);
        drop(app);

        let mut reopened = App::new(
            &fixture.path,
            Config::default(),
            Picker::halfblocks(),
            state_path,
        )
        .unwrap();
        screen(&mut reopened);
        assert_eq!(reopened.bookmarks.len(), 1);
        press(&mut reopened, KeyCode::Char('\''));
        assert!(screen(&mut reopened).contains("note"));
        press(&mut reopened, KeyCode::Char('x'));
        assert!(screen(&mut reopened).contains("bookmark deleted"));
        assert!(reopened.bookmarks.is_empty());
        assert!(screen(&mut reopened).contains("no bookmarks"));
    }

    #[test]
    fn shows_reading_stats() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let text = screen(&mut app);
        assert!(text.contains('~'));

        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.mode(), Mode::Stats);
        let text = screen(&mut app);
        assert!(text.contains("Time left"));
        assert!(text.contains("Words left"));
        assert!(text.contains("Reading stats"));
        assert!(text.contains("esc close"));

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn time_left_falls_while_reading() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        let (_, before) = app.reading_seconds();
        app.scroll_by(10);
        let (_, after) = app.reading_seconds();
        assert!(after <= before);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(3600), "1h 0m");
        assert_eq!(format_duration(59), "59s");
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn scroll_mouse(app: &mut App, up: bool) {
        app.handle_mouse(MouseEvent {
            kind: if up {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            },
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn mouse_opens_a_contents_entry() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('t'));
        screen(&mut app);

        let area = app.toc_list_area.expect("contents list is drawn");
        click(&mut app, area.x + 2, area.y + 1);

        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.chapter(), 1);
    }

    #[test]
    fn mouse_applies_a_theme() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press_ctrl(&mut app, KeyCode::Char('t'));
        screen(&mut app);
        let before = app.theme_name().to_string();

        let area = app.theme_list_area.expect("theme list is drawn");
        click(&mut app, area.x + 2, area.y + 1);

        assert_eq!(app.mode(), Mode::Normal);
        assert_ne!(app.theme_name(), before);
    }

    #[test]
    fn mouse_wheel_scrolls_the_page() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app);

        scroll_mouse(&mut app, false);
        let down = app.scroll();
        assert!(down > 0);
        scroll_mouse(&mut app, true);
        assert!(app.scroll() < down);
    }

    #[test]
    fn mouse_does_not_open_entries_outside_the_list() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('t'));
        screen(&mut app);

        click(&mut app, 0, 0);

        assert_eq!(app.mode(), Mode::Toc);
        assert_eq!(app.chapter(), 0);
    }

    #[test]
    fn saves_settings_with_s() {
        let fixture = crate::testutil::sample_book();
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        std::fs::write(&config_path, "# keep\n").unwrap();
        let state_path = directory.path().join("positions.toml");
        let mut app = App::new(
            &fixture.path,
            Config {
                path: Some(config_path.clone()),
                ..Config::default()
            },
            Picker::halfblocks(),
            state_path,
        )
        .unwrap();
        screen(&mut app);

        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('S'));

        assert!(screen(&mut app).contains("settings saved to"));
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(written.contains("# keep"));
        assert!(written.contains("images = false"));
        assert!(written.contains("paragraph_colors = true"));
    }

    #[test]
    fn help_lists_the_new_keys() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.mode(), Mode::Help);
        let text = screen(&mut app);
        for needle in [
            "next link",
            "follow or open",
            "back after link",
            "add bookmark",
            "bookmarks",
            "reading time",
            "save settings",
            "toggle paragraph colors",
            "pull latest sync position",
            "wheel, click popups",
        ] {
            assert!(text.contains(needle), "help is missing {needle}");
        }
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn saves_whole_book_progress() {
        let fixture = crate::testutil::sample_book();
        let (mut app, state_path, _directory) = app(&fixture);
        screen(&mut app);
        press(&mut app, KeyCode::Char('%'));
        type_text(&mut app, "50");
        press(&mut app, KeyCode::Enter);
        app.save_position().unwrap();

        let store = Store::load(&state_path);
        let position = store.get(&app.book_key).expect("position is saved");
        assert_eq!(position.chapters, app.book.chapters.len());
        assert!(
            (position.book_progress - 0.5).abs() < 0.05,
            "book progress was {}",
            position.book_progress
        );
        let percent = (position.book_progress * 100.0).round() as i32;
        assert!((percent - i32::from(app.book_percent())).abs() <= 1);
    }

    #[test]
    fn theme_selector_previews_and_restores() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let before = app.theme_name().to_string();

        press_ctrl(&mut app, KeyCode::Char('t'));
        assert_eq!(app.mode(), Mode::Theme);
        type_text(&mut app, "nord");
        let text = screen(&mut app);
        assert!(text.contains("nord"));
        assert_eq!(app.theme_name(), "nord");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.theme_name(), before);
    }

    #[test]
    fn theme_selector_filters_and_applies() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);

        press_ctrl(&mut app, KeyCode::Char('t'));
        type_text(&mut app, "tokyo");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.theme_name(), "tokyo-night-moon");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.theme_name(), "tokyo-night-moon");
    }

    #[test]
    fn title_line_keeps_the_author() {
        let line = title_line(
            "A very long book title that will not fit on one line",
            Some("Author Name"),
            30,
        );
        assert!(line.contains("Author Name"));
        assert!(layout::display_width(&line) <= 30);
        assert!(title_line("Short", Some("Author"), 40).contains("Author"));
        assert!(title_line("Short", None, 40).contains("Short"));
    }

    #[test]
    fn status_line_background_differs_from_the_page() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        let status = buffer[(10, 29)].bg;
        let body = buffer[(10, 10)].bg;
        assert_ne!(status, body);
    }

    #[test]
    fn status_line_shows_title_and_author() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        let text = screen(&mut app);
        assert!(text.contains("Test Book"));
        assert!(text.contains("Test Author"));
        assert!(!text.contains("silkcircuit-neon"));
    }

    #[test]
    fn theme_filter_accepts_j_and_k() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);

        press_ctrl(&mut app, KeyCode::Char('t'));
        type_text(&mut app, "kanagawa-wave");
        assert_eq!(app.theme_name(), "kanagawa-wave");
    }

    #[test]
    fn opens_table_of_contents() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        screen(&mut app);

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.mode(), Mode::Toc);
        let text = screen(&mut app);
        assert!(text.contains("First Chapter"));
        assert!(text.contains("Part Two"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.chapter(), 1);
    }

    #[test]
    fn toc_can_be_filtered() {
        let fixture = crate::testutil::sample_book();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('t'));
        type_text(&mut app, "ptw");

        let text = screen(&mut app);
        assert!(text.contains("filter: ptw"));
        assert!(text.contains("Part Two"));

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.chapter(), 1);

        press(&mut app, KeyCode::Char('t'));
        type_text(&mut app, "zzz");
        assert!(screen(&mut app).contains("no matching entries"));
        press(&mut app, KeyCode::Esc);
    }

    #[test]
    fn shows_message_when_toc_is_empty() {
        let fixture = crate::testutil::sample_book_without_toc();
        let (mut app, _, _directory) = app(&fixture);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.mode(), Mode::Toc);
        assert!(screen(&mut app).contains("No table of contents"));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode(), Mode::Normal);
    }
}
