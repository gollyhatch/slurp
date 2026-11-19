//! Application state, input handling, and drawing.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::book::Book;
use crate::config::Config;
use crate::layout::{self, Options, View};
use crate::state::{self, BookPosition, Store};
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
    toc_selected: usize,
    toc_offset: usize,
    theme_matches: Vec<usize>,
    theme_filter: String,
    theme_selected: usize,
    theme_offset: usize,
    theme_before: Option<(String, Palette)>,
    images: bool,
    message: Option<String>,
    quit: bool,
    image_dimensions: HashMap<String, (u32, u32)>,
    image_protocols: HashMap<(String, u16, u16), Protocol>,
    state: Store,
    state_path: PathBuf,
    book_key: String,
    pending_progress: Option<f32>,
    chapter_chars: Vec<usize>,
    total_chars: usize,
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

        let book_key = fs::canonicalize(book_path)
            .unwrap_or_else(|_| book_path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let store = Store::load(&state_path);
        let saved = store.get(&book_key).cloned();

        let chapter = saved
            .as_ref()
            .map(|position| position.chapter)
            .unwrap_or(0)
            .min(book.chapters.len() - 1);

        let images = config.images;
        let content_width = config.text_width;
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
            chapter,
            scroll: 0,
            laid_out: false,
            mode: Mode::Normal,
            input: String::new(),
            search: None,
            search_before: None,
            toc_selected: chapter,
            toc_offset: 0,
            theme_matches: Vec::new(),
            theme_filter: String::new(),
            theme_selected: 0,
            theme_offset: 0,
            theme_before: None,
            message: None,
            quit: false,
            image_dimensions: HashMap::new(),
            image_protocols: HashMap::new(),
            state: store,
            state_path,
            book_key,
            pending_progress: saved.map(|position| position.progress),
            chapter_chars,
            total_chars,
        };
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
        self.state.set(BookPosition {
            path: self.book_key.clone(),
            title: self.book.title.clone(),
            chapter: self.chapter,
            progress,
            updated: state::now(),
        });
        self.state.save(&self.state_path)
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

    /// Draw the reader.
    pub fn draw(&mut self, frame: &mut Frame) {
        self.ensure_size(frame.area());

        let area = frame.area();
        if let Some((_, top, rows)) = self.view.item_at_row(self.scroll) {
            if rows > 1 && top < self.scroll {
                self.scroll = top;
            }
        }
        frame.render_widget(
            Block::new().style(Style::default().bg(self.palette.background)),
            area,
        );
        if area.width == 0 || area.height == 0 {
            return;
        }

        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

        self.draw_header(frame, header_area);
        self.draw_body(frame, body_area);
        self.draw_footer(frame, footer_area);

        match self.mode {
            Mode::Toc => self.draw_toc(frame, area),
            Mode::Help => self.draw_help(frame, area),
            Mode::Theme => self.draw_theme_selector(frame, area),
            _ => {}
        }

        let prompt = match self.mode {
            Mode::Search => Some(format!("/ {}", self.input)),
            Mode::Percent => Some(format!("goto {}", self.input)),
            _ => None,
        };
        if let Some(prompt) = prompt {
            let x = footer_area
                .x
                .saturating_add(1)
                .saturating_add(layout::display_width(&prompt) as u16);
            if x < footer_area.right() {
                frame.set_cursor_position(Position::new(x, footer_area.y));
            }
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        let half = (self.viewport_rows / 2).max(1) as i64;
        let page = self.viewport_rows.max(1) as i64;

        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                let _ = self.save_position();
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('d') => self.scroll_by(half),
            KeyCode::Char('u') => self.scroll_by(-half),
            KeyCode::Char(' ') => self.next_paragraph(),
            KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => {
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
                self.toc_selected = self.chapter;
                self.mode = Mode::Toc;
            }
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
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_width(4),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_width(-4),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Esc => {
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

        self.chapter = chapter;
        self.toc_selected = chapter;
        self.scroll = 0;
        self.rebuild_view();
        let max = self.max_scroll();
        let row = (fraction * self.view.total_rows as f32).round() as usize;
        self.scroll = self.snap(row.min(max), true).min(max);
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

    fn key_toc(&mut self, key: KeyEvent) {
        let last = self.book.chapters.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('q') => {
                self.mode = Mode::Normal
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.toc_selected = (self.toc_selected + 1).min(last);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.toc_selected = self.toc_selected.saturating_sub(1);
            }
            KeyCode::Char('g') | KeyCode::Home => self.toc_selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.toc_selected = last,
            KeyCode::PageDown => {
                self.toc_selected = (self.toc_selected + 10).min(last);
            }
            KeyCode::PageUp => {
                self.toc_selected = self.toc_selected.saturating_sub(10);
            }
            KeyCode::Enter => {
                let selected = self.toc_selected;
                self.mode = Mode::Normal;
                self.open_chapter(selected);
            }
            _ => {}
        }
    }

    fn open_chapter(&mut self, index: usize) {
        let index = index.min(self.book.chapters.len() - 1);
        if index == self.chapter {
            return;
        }
        self.chapter = index;
        self.scroll = 0;
        self.toc_selected = index;
        self.rebuild_view();
        let _ = self.save_position();
    }

    fn next_paragraph(&mut self) {
        let max = self.max_scroll();
        self.scroll = match self.view.next_paragraph_row(self.scroll) {
            Some(top) => top.min(max),
            None => max,
        };
    }

    fn scroll_by(&mut self, delta: i64) {
        if self.view.total_rows <= usize::from(self.viewport_rows) {
            self.scroll = 0;
            return;
        }
        let max = self.max_scroll() as i64;
        let target = (self.scroll as i64 + delta).clamp(0, max) as usize;
        self.scroll = self.snap(target, delta >= 0).min(max as usize);
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
            self.chapter = hit.chapter;
            self.scroll = 0;
            self.rebuild_view();
        }
        if let Some(index) = self.view.find_hit(hit.block, &query, hit.offset) {
            let top = self.view.tops[index];
            let margin = usize::from(self.viewport_rows) / 3;
            let target = top.saturating_sub(margin);
            self.scroll = self.snap(target, true).min(self.max_scroll());
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
            KeyCode::Enter => {
                self.theme_before = None;
                self.mode = Mode::Normal;
                let name = theme::builtin_names()
                    .iter()
                    .find(|(id, _)| *id == self.config.theme)
                    .map(|(_, name)| *name)
                    .unwrap_or(self.config.theme.as_str());
                self.message = Some(format!("theme: {name}"));
            }
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

    fn ensure_size(&mut self, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content = self.content_width_for(area);
        let viewport = area.height.saturating_sub(2).max(1);
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
            let max = self.max_scroll();
            self.scroll = self.snap(target.min(max), true).min(max);
        }
    }

    fn rebuild_view(&mut self) {
        let options = Options {
            width: self.content_width,
            paragraph_spacing: self.config.paragraph_spacing,
            image_max_rows: self
                .config
                .image_max_height
                .min(self.viewport_rows.saturating_sub(2).max(2)),
            images: self.images,
            query: self.search.as_ref().map(|search| search.query.clone()),
        };

        let book = &self.book;
        let chapter = &book.chapters[self.chapter];
        let palette = &self.palette;
        let picker = &self.picker;
        let dimensions = &mut self.image_dimensions;

        let view = layout::build(chapter, palette, &options, picker, |src| {
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
        });

        self.view = view;
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let left = format!(" {}", self.book.title);
        let left = truncate(&left, usize::from(area.width));
        let right = format!("{} ", self.config.theme);
        let right = truncate(&right, usize::from(area.width));
        let line = bar_line(&left, &right, area.width, self.palette.status());
        frame.render_widget(Paragraph::new(line), area);
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
            _ => {}
        }

        let progress = self.book_percent();
        let mut left = if let Some(message) = &self.message {
            format!(" {message}")
        } else {
            format!(
                " {}/{} {}",
                self.chapter + 1,
                self.book.chapters.len(),
                self.book.chapters[self.chapter].title
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
        let right = format!("{progress}%  ? help ");
        let line = bar_line(&left, &right, area.width, self.palette.status());
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_body(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Block::new().style(self.palette.base()), area);
        if self.view.items.is_empty() {
            return;
        }

        let width = usize::from(area.width);
        let content_width = usize::from(self.content_width).min(width).max(1);
        let x = area.x + ((width - content_width) / 2) as u16;

        let mut index = self
            .view
            .item_at_row(self.scroll)
            .map(|(index, _, _)| index)
            .unwrap_or(0);
        let mut y = area.y;

        while index < self.view.items.len() && y < area.bottom() {
            let top = self.view.tops[index];
            let rows = u16::try_from(self.view.rows(index)).unwrap_or(u16::MAX);
            if top < self.scroll {
                index += 1;
                continue;
            }
            if y + rows > area.bottom() {
                break;
            }

            let line = if let layout::Item::Line { line, .. } =
                &self.view.items[index]
            {
                Some(line.clone())
            } else {
                None
            };
            let image = if let layout::Item::Image { key, alt, cols, .. } =
                &self.view.items[index]
            {
                Some((key.clone(), alt.clone(), *cols))
            } else {
                None
            };

            if let Some(line) = line {
                let rect = Rect::new(x, y, content_width as u16, 1);
                frame.render_widget(Paragraph::new(line), rect);
            } else if let Some((key, alt, image_cols)) = image {
                let image_width = image_cols.min(content_width as u16).max(1);
                let image_x = x + ((content_width as u16 - image_width) / 2);
                let rect = Rect::new(
                    image_x,
                    y,
                    image_width,
                    rows.min(area.bottom() - y),
                );
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
                        Paragraph::new(label).style(self.palette.muted()),
                        rect,
                    );
                }
            }

            y += rows;
            index += 1;
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
        if area.width < 12 || area.height < 5 {
            return;
        }
        let width = 72.min(area.width.saturating_sub(4)).max(20);
        let height = (self.book.chapters.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(4);
        let rect = centered(area, width, height);
        frame.render_widget(Clear, rect);

        let block = Block::bordered()
            .title(" Contents ")
            .border_style(self.palette.border_style())
            .style(self.palette.base());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let visible = usize::from(inner.height);
        if self.toc_selected < self.toc_offset {
            self.toc_offset = self.toc_selected;
        } else if visible > 0 && self.toc_selected >= self.toc_offset + visible
        {
            self.toc_offset = self.toc_selected + 1 - visible;
        }

        let mut lines = Vec::new();
        for (index, chapter) in self
            .book
            .chapters
            .iter()
            .enumerate()
            .skip(self.toc_offset)
            .take(visible)
        {
            let style = if index == self.toc_selected {
                self.palette.selected()
            } else {
                self.palette.base()
            };
            let marker = if index == self.chapter {
                "\u{25b8}"
            } else {
                " "
            };
            lines.push(Line::from(vec![Span::styled(
                format!("{marker} {:>3}  {}", index + 1, chapter.title),
                style,
            )]));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_theme_selector(&mut self, frame: &mut Frame, area: Rect) {
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

        frame.render_widget(
            Paragraph::new(Line::styled(
                " enter apply   esc cancel   up/down select ",
                self.palette.muted(),
            )),
            hint_area,
        );
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        const HELP: &[&str] = &[
            " j / k        line down / up",
            " d / u        half page down / up",
            " Space        next paragraph",
            " PgDn / PgUp  page down / up",
            " b            page up",
            " g / G        start / end of chapter",
            " h / l        previous / next chapter",
            " t            table of contents",
            " %            jump to a percentage",
            " /            full-text search",
            " n / N        next / previous match",
            " i            toggle images",
            " Ctrl+T       theme selector",
            " + / -        wider / narrower text",
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

fn bar_line(
    left: &str,
    right: &str,
    width: u16,
    style: Style,
) -> Line<'static> {
    let width = usize::from(width);
    let left = truncate(left, width);
    let right = truncate(right, width);
    let used = layout::display_width(&left) + layout::display_width(&right);
    let padding = width.saturating_sub(used).max(1);
    Line::from(vec![Span::styled(
        format!("{left}{}{right}", " ".repeat(padding)),
        style,
    )])
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
        assert!(screen(&mut app).contains("tokyo-night-moon"));
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
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.chapter(), 1);
    }
}
