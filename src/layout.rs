//! Wrap chapter blocks into rows for the terminal.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::book::Chapter;
use crate::html::{Block, Inline};
use crate::theme::Palette;

/// A character with the style used to draw it.
#[derive(Debug, Clone, Copy)]
pub struct StyledChar {
    pub ch: char,
    pub style: Style,
}

/// Options for building a view.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub width: u16,
    pub paragraph_spacing: u16,
    pub image_max_rows: u16,
    /// Set to `false` to render images as text placeholders.
    pub images: bool,
    /// Empty rows appended after the last block, used to separate chapters.
    pub trailing_blank_rows: u16,
    pub query: Option<String>,
}

/// One rendered row group.
pub enum Item {
    Line {
        line: Line<'static>,
        plain: String,
        block: usize,
    },
    Image {
        key: String,
        alt: String,
        rows: u16,
        cols: u16,
        block: usize,
    },
}

impl Item {
    /// Number of terminal rows this item uses.
    pub fn rows(&self) -> u16 {
        match self {
            Item::Line { .. } => 1,
            Item::Image { rows, .. } => *rows,
        }
    }

    /// Index of the source block.
    pub fn block(&self) -> usize {
        match self {
            Item::Line { block, .. } | Item::Image { block, .. } => *block,
        }
    }
}

/// A chapter laid out for the terminal.
pub struct View {
    pub items: Vec<Item>,
    /// Top row of each item, plus the total height as the last entry.
    pub tops: Vec<usize>,
    pub total_rows: usize,
    /// First item index per source block.
    pub block_first_item: Vec<usize>,
    /// Item indexes that start a paragraph-like block.
    pub paragraph_starts: Vec<usize>,
    pub width: u16,
}

impl View {
    /// An empty view.
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            tops: vec![0],
            total_rows: 0,
            block_first_item: Vec::new(),
            paragraph_starts: Vec::new(),
            width: 0,
        }
    }

    /// Row of the first paragraph start below `row`.
    pub fn next_paragraph_row(&self, row: usize) -> Option<usize> {
        self.paragraph_starts
            .iter()
            .map(|index| self.tops[*index])
            .find(|top| *top > row)
    }

    /// Rows used by an item.
    pub fn rows(&self, index: usize) -> usize {
        self.tops[index + 1] - self.tops[index]
    }

    /// Item that contains a row.
    pub fn item_at_row(&self, row: usize) -> Option<(usize, usize, usize)> {
        if self.items.is_empty() {
            return None;
        }
        let mut low = 0;
        let mut high = self.items.len();
        while low < high {
            let middle = (low + high) / 2;
            if self.tops[middle + 1] <= row {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let index = low.min(self.items.len() - 1);
        Some((index, self.tops[index], self.rows(index)))
    }

    /// Source block at a row.
    pub fn block_at_row(&self, row: usize) -> usize {
        self.item_at_row(row)
            .map(|(index, _, _)| self.items[index].block())
            .unwrap_or(0)
    }

    /// Find the item that shows a search hit.
    ///
    /// `offset` is the byte offset of the hit inside the plain text of the
    /// block. It is used to choose between several hits in the same block.
    pub fn find_hit(
        &self,
        block: usize,
        query: &str,
        offset: usize,
    ) -> Option<usize> {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return self.block_first_item.get(block).copied();
        }

        let mut first = None;
        let mut best = None;
        let mut seen = 0;
        for (index, item) in self.items.iter().enumerate() {
            let Item::Line {
                plain,
                block: item_block,
                ..
            } = item
            else {
                continue;
            };
            if *item_block != block {
                continue;
            }
            let haystack = plain.to_lowercase();
            if let Some(position) = haystack.find(&needle) {
                first.get_or_insert(index);
                if best.is_none() && seen + position >= offset {
                    best = Some(index);
                }
            }
            seen += plain.chars().count() + 1;
        }

        best.or(first)
            .or_else(|| self.block_first_item.get(block).copied())
    }
}

/// Lay out a chapter.
pub fn build(
    chapter: &Chapter,
    palette: &Palette,
    options: &Options,
    picker: &Picker,
    mut image_size: impl FnMut(&str) -> Option<(String, u32, u32)>,
) -> View {
    let width = usize::from(options.width).max(1);
    let spacing = usize::from(options.paragraph_spacing);
    let max_image_rows = options.image_max_rows.clamp(2, 200);
    let query = options.query.as_deref();
    let highlight = palette.highlight();

    let mut items: Vec<Item> = Vec::new();
    let mut block_first_item = Vec::with_capacity(chapter.blocks.len());
    let mut paragraph_starts = Vec::new();
    let mut last_was_list = false;

    for (index, block) in chapter.blocks.iter().enumerate() {
        let is_line = matches!(block, Block::Line(_));
        let is_list = matches!(block, Block::ListItem { .. });
        let is_heading = matches!(block, Block::Heading { .. });

        if !is_line {
            let mut gap = if is_list && last_was_list { 0 } else { spacing };
            if is_heading {
                gap += 1;
            }
            push_blanks(&mut items, gap, index);
        }

        let start_index = items.len();
        block_first_item.push(start_index);

        match block {
            Block::Heading { level, content } => {
                let style = if *level <= 2 {
                    palette.heading()
                } else {
                    palette.heading().add_modifier(Modifier::ITALIC)
                };
                push_wrapped(
                    &mut items,
                    index,
                    content,
                    style,
                    "",
                    "",
                    palette.base(),
                    palette.code(),
                    palette.accent,
                    width,
                    query,
                    highlight,
                );
            }
            Block::Paragraph(content) => push_wrapped(
                &mut items,
                index,
                content,
                palette.base(),
                "",
                "",
                palette.base(),
                palette.code(),
                palette.accent,
                width,
                query,
                highlight,
            ),
            Block::Line(content) => push_wrapped(
                &mut items,
                index,
                content,
                palette.base(),
                "",
                "",
                palette.base(),
                palette.code(),
                palette.accent,
                width,
                query,
                highlight,
            ),
            Block::Quote(content) => push_wrapped(
                &mut items,
                index,
                content,
                palette.muted().add_modifier(Modifier::ITALIC),
                "\u{2502} ",
                "  ",
                palette.muted(),
                palette.code(),
                palette.accent,
                width,
                query,
                highlight,
            ),
            Block::ListItem {
                marker,
                depth,
                content,
            } => {
                let indent = "  ".repeat(depth.saturating_sub(1));
                let first = format!("{indent}{marker} ");
                let rest = " ".repeat(display_width(&first));
                push_wrapped(
                    &mut items,
                    index,
                    content,
                    palette.base(),
                    &first,
                    &rest,
                    palette.muted(),
                    palette.code(),
                    palette.accent,
                    width,
                    query,
                    highlight,
                );
            }
            Block::Pre(content) => {
                let text =
                    crate::html::plain_text(content).replace('\t', "    ");
                for raw in text.split('\n') {
                    let chars = truncate_line(raw, width, palette.code());
                    let plain: String =
                        chars.iter().map(|character| character.ch).collect();
                    let mut chars = chars;
                    if let Some(query) = query {
                        apply_highlight(&mut chars, query, highlight);
                    }
                    items.push(Item::Line {
                        line: chars_to_line(chars),
                        plain,
                        block: index,
                    });
                }
            }
            Block::Rule => {
                let chars: Vec<StyledChar> = "\u{2500}"
                    .repeat(width)
                    .chars()
                    .map(|ch| StyledChar {
                        ch,
                        style: palette.border_style(),
                    })
                    .collect();
                items.push(Item::Line {
                    line: chars_to_line(chars),
                    plain: String::new(),
                    block: index,
                });
            }
            Block::Image { src, alt } => {
                let size = if options.images {
                    image_size(src)
                } else {
                    None
                };
                match size {
                    Some((key, pixel_width, pixel_height)) => {
                        let font = picker.font_size();
                        let (cols, rows) = fit_image_size(
                            pixel_width,
                            pixel_height,
                            options.width.max(1),
                            max_image_rows,
                            font.width,
                            font.height,
                        );
                        items.push(Item::Image {
                            key,
                            alt: alt.clone(),
                            rows,
                            cols,
                            block: index,
                        });
                    }
                    None => {
                        let label = if alt.is_empty() {
                            "[image]".to_string()
                        } else {
                            format!("[image: {alt}]")
                        };
                        push_wrapped(
                            &mut items,
                            index,
                            &[Inline {
                                text: label,
                                flags: Default::default(),
                            }],
                            palette.muted(),
                            "",
                            "",
                            palette.muted(),
                            palette.code(),
                            palette.accent,
                            width,
                            query,
                            highlight,
                        );
                    }
                }
            }
        }

        if !is_line
            && !matches!(block, Block::Rule)
            && items.len() > start_index
        {
            paragraph_starts.push(start_index);
        }
        last_was_list = is_list;
    }

    if options.trailing_blank_rows > 0 && !items.is_empty() {
        let block = chapter.blocks.len().saturating_sub(1);
        for _ in 0..options.trailing_blank_rows {
            items.push(Item::Line {
                line: Line::default(),
                plain: String::new(),
                block,
            });
        }
    }

    let mut tops = Vec::with_capacity(items.len() + 1);
    let mut row = 0;
    for item in &items {
        tops.push(row);
        row += usize::from(item.rows());
    }
    tops.push(row);

    View {
        items,
        tops,
        total_rows: row,
        block_first_item,
        paragraph_starts,
        width: options.width,
    }
}

fn push_blanks(items: &mut Vec<Item>, count: usize, block: usize) {
    for _ in 0..count {
        if items.is_empty() {
            break;
        }
        if matches!(items.last(), Some(Item::Line { plain, .. }) if plain.is_empty())
        {
            continue;
        }
        items.push(Item::Line {
            line: Line::default(),
            plain: String::new(),
            block,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn push_wrapped(
    items: &mut Vec<Item>,
    block: usize,
    inlines: &[Inline],
    style: Style,
    prefix_first: &str,
    prefix_rest: &str,
    prefix_style: Style,
    code_style: Style,
    link_color: Color,
    width: usize,
    query: Option<&str>,
    highlight: Style,
) {
    let chars = inline_chars(inlines, style, code_style, link_color);
    if chars.is_empty() {
        return;
    }

    let prefix_width =
        display_width(prefix_first).max(display_width(prefix_rest));
    let available = width.saturating_sub(prefix_width).max(1);
    let lines = wrap(&chars, available);

    for (number, mut chars) in lines.into_iter().enumerate() {
        let plain: String =
            chars.iter().map(|character| character.ch).collect();
        if let Some(query) = query {
            apply_highlight(&mut chars, query, highlight);
        }
        let prefix = if number == 0 {
            prefix_first
        } else {
            prefix_rest
        };
        let mut full = prefix_chars(prefix, prefix_style);
        full.extend(chars);
        items.push(Item::Line {
            line: chars_to_line(full),
            plain,
            block,
        });
    }
}

fn inline_chars(
    inlines: &[Inline],
    style: Style,
    code_style: Style,
    link_color: Color,
) -> Vec<StyledChar> {
    let mut chars = Vec::new();
    for inline in inlines {
        let mut style = style;
        if inline.flags.code {
            style = code_style;
        }
        if inline.flags.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if inline.flags.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if inline.flags.link {
            style = style.fg(link_color).add_modifier(Modifier::UNDERLINED);
        }
        for ch in inline.text.chars() {
            chars.push(StyledChar { ch, style });
        }
    }
    chars
}

fn prefix_chars(prefix: &str, style: Style) -> Vec<StyledChar> {
    prefix.chars().map(|ch| StyledChar { ch, style }).collect()
}

/// Wrap styled characters into lines of at most `width` columns.
///
/// Explicit newlines start a new line. Spaces at the start of a line are
/// dropped.
pub fn wrap(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<StyledChar> = Vec::new();
    let mut line_width = 0;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];

        if character.ch == '\n' {
            trim_end(&mut line);
            lines.push(std::mem::take(&mut line));
            line_width = 0;
            index += 1;
            continue;
        }

        if character.ch == ' ' {
            if line.is_empty() {
                index += 1;
                continue;
            }
            let next_word = word_width(chars, index + 1);
            if line_width + 1 + next_word > width {
                trim_end(&mut line);
                lines.push(std::mem::take(&mut line));
                line_width = 0;
                index += 1;
                continue;
            }
            line.push(character);
            line_width += 1;
            index += 1;
            continue;
        }

        let char_width = character_width(character.ch);
        if line_width + char_width > width && !line.is_empty() {
            trim_end(&mut line);
            lines.push(std::mem::take(&mut line));
            line_width = 0;
            continue;
        }
        line.push(character);
        line_width += char_width;
        index += 1;
    }

    if !line.is_empty() {
        trim_end(&mut line);
        lines.push(line);
    }
    lines
}

fn word_width(chars: &[StyledChar], start: usize) -> usize {
    let mut width = 0;
    for character in &chars[start.min(chars.len())..] {
        if character.ch == ' ' || character.ch == '\n' {
            break;
        }
        width += character_width(character.ch);
    }
    width
}

fn trim_end(line: &mut Vec<StyledChar>) {
    while matches!(line.last(), Some(character) if character.ch == ' ') {
        line.pop();
    }
}

/// Fit an image into a cell grid, preserving its aspect ratio.
fn fit_image_size(
    pixel_width: u32,
    pixel_height: u32,
    max_cols: u16,
    max_rows: u16,
    font_width: u16,
    font_height: u16,
) -> (u16, u16) {
    let image_width = f64::from(pixel_width.max(1));
    let image_height = f64::from(pixel_height.max(1));
    let cell_width = f64::from(font_width.max(1));
    let cell_height = f64::from(font_height.max(1));
    let max_cols = f64::from(max_cols.max(1));
    let max_rows = f64::from(max_rows.max(1));

    let mut cols = (image_width / cell_width).ceil().clamp(1.0, max_cols);
    let mut rows = (cols * image_height / image_width * cell_width
        / cell_height)
        .ceil()
        .max(1.0);
    if rows > max_rows {
        rows = max_rows;
        cols = (rows * image_width / image_height * cell_height / cell_width)
            .floor()
            .clamp(1.0, max_cols);
    }

    (cols as u16, rows as u16)
}

/// Screen width of a character, in cells.
pub fn character_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// Screen width of a string, in cells.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate_line(text: &str, width: usize, style: Style) -> Vec<StyledChar> {
    let mut chars = Vec::new();
    let mut used = 0;
    for ch in text.chars() {
        let char_width = character_width(ch);
        if used + char_width > width {
            break;
        }
        used += char_width;
        chars.push(StyledChar { ch, style });
    }
    chars
}

/// Mark every case-insensitive occurrence of `query`.
fn apply_highlight(chars: &mut [StyledChar], query: &str, style: Style) {
    let needle: Vec<char> = query.chars().map(lower_char).collect();
    if needle.is_empty() {
        return;
    }
    let haystack: Vec<char> = chars
        .iter()
        .map(|character| lower_char(character.ch))
        .collect();

    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        if haystack[start..start + needle.len()] == needle[..] {
            for character in &mut chars[start..start + needle.len()] {
                character.style = style;
            }
            start += needle.len().max(1);
        } else {
            start += 1;
        }
    }
}

fn lower_char(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

fn chars_to_line(chars: Vec<StyledChar>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut style = Style::default();

    for character in chars {
        if current.is_empty() {
            style = character.style;
        } else if character.style != style {
            spans.push(Span::styled(std::mem::take(&mut current), style));
            style = character.style;
        }
        current.push(character.ch);
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::html::{Inline, InlineFlags};

    fn words(text: &str) -> Vec<StyledChar> {
        text.chars()
            .map(|ch| StyledChar {
                ch,
                style: Style::default(),
            })
            .collect()
    }

    #[test]
    fn wraps_at_width() {
        let chars = words("the quick brown fox jumps over the lazy dog");
        let lines = wrap(&chars, 16);
        assert!(lines.len() > 1);
        for line in &lines {
            let width: usize = line.iter().map(|c| character_width(c.ch)).sum();
            assert!(width <= 16, "line too wide: {width}");
        }
        let joined: String = lines
            .iter()
            .map(|line| line.iter().map(|c| c.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined, "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn breaks_long_words() {
        let chars = words("abcdefghijklmnopqrstuvwxyz");
        let lines = wrap(&chars, 10);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), 10);
    }

    #[test]
    fn keeps_explicit_newlines() {
        let chars = words("one\ntwo\n\nthree");
        let lines = wrap(&chars, 40);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|c| c.ch).collect())
            .collect();
        assert_eq!(rendered, vec!["one", "two", "", "three"]);
    }

    #[test]
    fn highlights_matches() {
        let mut chars = words("Alice and alice");
        apply_highlight(
            &mut chars,
            "alice",
            Style::default().fg(ratatui::style::Color::Red),
        );
        assert_eq!(chars[0].style.fg, Some(ratatui::style::Color::Red));
        assert_eq!(chars[10].style.fg, Some(ratatui::style::Color::Red));
        assert_eq!(chars[5].style.fg, None);
    }

    #[test]
    fn builds_view_from_blocks() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![
                Block::Heading {
                    level: 1,
                    content: vec![Inline {
                        text: "Title".into(),
                        flags: InlineFlags::default(),
                    }],
                },
                Block::Paragraph(vec![Inline {
                    text: "alpha beta gamma delta epsilon".into(),
                    flags: InlineFlags::default(),
                }]),
                Block::Image {
                    src: "missing.png".into(),
                    alt: "missing".into(),
                },
            ],
            plain: vec![
                "Title".into(),
                "alpha beta gamma delta epsilon".into(),
                "missing".into(),
            ],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 20,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: true,
            trailing_blank_rows: 0,
            query: None,
        };
        let picker = Picker::halfblocks();
        let view = build(&chapter, &palette, &options, &picker, |_| None);
        assert!(view.items.iter().any(|item| matches!(
            item,
            Item::Line { plain, .. } if plain.contains("[image: missing]")
        )));

        assert!(!view.items.is_empty());
        for (index, item) in view.items.iter().enumerate() {
            if let Item::Line { plain, .. } = item {
                assert!(display_width(plain) <= 20 || plain.is_empty());
            }
            assert_eq!(
                view.tops[index + 1] - view.tops[index],
                usize::from(item.rows())
            );
        }
        assert_eq!(view.block_first_item.len(), 3);
        assert!(view.find_hit(1, "gamma", 0).is_some());
        assert_eq!(view.paragraph_starts, vec![0, 2, 5]);
        assert_eq!(view.next_paragraph_row(0), Some(2));

        let highlighted = build(
            &chapter,
            &palette,
            &Options {
                query: Some("gamma".into()),
                ..options
            },
            &picker,
            |_| None,
        );
        let line = highlighted
            .items
            .iter()
            .find_map(|item| match item {
                Item::Line { line, plain, .. } if plain.contains("gamma") => {
                    Some(line)
                }
                _ => None,
            })
            .unwrap();
        assert!(line
            .spans
            .iter()
            .any(|span| span.style.bg == Some(ratatui::style::Color::Red)));
    }

    #[test]
    fn appends_trailing_blank_rows() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![Block::Paragraph(vec![Inline {
                text: "one".into(),
                flags: InlineFlags::default(),
            }])],
            plain: vec!["one".into()],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 40,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: true,
            trailing_blank_rows: 2,
            query: None,
        };
        let view =
            build(&chapter, &palette, &options, &Picker::halfblocks(), |_| {
                None
            });
        assert_eq!(view.total_rows, 3);
        let blanks = view.items.iter().rev().take(2);
        for item in blanks {
            match item {
                Item::Line { plain, .. } => assert!(plain.is_empty()),
                Item::Image { .. } => panic!("expected blank lines"),
            }
        }
    }

    #[test]
    fn skips_soft_line_breaks_in_paragraph_navigation() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![
                Block::Paragraph(vec![Inline {
                    text: "one".into(),
                    flags: InlineFlags::default(),
                }]),
                Block::Line(vec![Inline {
                    text: "line two".into(),
                    flags: InlineFlags::default(),
                }]),
                Block::Paragraph(vec![Inline {
                    text: "three".into(),
                    flags: InlineFlags::default(),
                }]),
            ],
            plain: vec!["one".into(), "line two".into(), "three".into()],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 40,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: true,
            trailing_blank_rows: 0,
            query: None,
        };
        let view =
            build(&chapter, &palette, &options, &Picker::halfblocks(), |_| {
                None
            });
        assert_eq!(view.paragraph_starts, vec![0, 3]);
        assert_eq!(view.next_paragraph_row(0), Some(3));
        assert_eq!(view.next_paragraph_row(1), Some(3));
        assert_eq!(view.next_paragraph_row(3), None);
    }

    #[test]
    fn renders_italic_text() {
        let inlines = vec![
            Inline {
                text: "plain ".into(),
                flags: InlineFlags::default(),
            },
            Inline {
                text: "slanted".into(),
                flags: InlineFlags {
                    italic: true,
                    ..InlineFlags::default()
                },
            },
            Inline {
                text: " linked".into(),
                flags: InlineFlags {
                    italic: true,
                    link: true,
                    ..InlineFlags::default()
                },
            },
        ];
        let chars = inline_chars(
            &inlines,
            Style::default(),
            Style::default(),
            ratatui::style::Color::Cyan,
        );

        let slanted: String = chars
            .iter()
            .filter(|c| c.style.add_modifier.contains(Modifier::ITALIC))
            .map(|c| c.ch)
            .collect();
        assert_eq!(slanted, "slanted linked");

        let linked = chars.last().expect("last character");
        assert!(linked.style.add_modifier.contains(Modifier::ITALIC));
        assert!(linked.style.add_modifier.contains(Modifier::UNDERLINED));
        let plain = chars.first().expect("first character");
        assert!(!plain.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn underlines_link_text() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![Block::Paragraph(vec![
                Inline {
                    text: "see ".into(),
                    flags: InlineFlags::default(),
                },
                Inline {
                    text: "note".into(),
                    flags: InlineFlags {
                        link: true,
                        ..InlineFlags::default()
                    },
                },
            ])],
            plain: vec!["see note".into()],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 40,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: true,
            trailing_blank_rows: 0,
            query: None,
        };
        let view =
            build(&chapter, &palette, &options, &Picker::halfblocks(), |_| {
                None
            });
        let line = view
            .items
            .iter()
            .find_map(|item| match item {
                Item::Line { line, .. } => Some(line),
                _ => None,
            })
            .unwrap();
        assert!(line.spans.iter().any(|span| {
            span.content.contains("note")
                && span.style.add_modifier.contains(Modifier::UNDERLINED)
                && span.style.fg == Some(ratatui::style::Color::Cyan)
        }));
        assert!(line.spans.iter().any(|span| {
            span.content.contains("see")
                && span.style.fg == Some(ratatui::style::Color::White)
        }));
    }

    #[test]
    fn sizes_images_by_aspect_ratio() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![Block::Image {
                src: "pic.png".into(),
                alt: String::new(),
            }],
            plain: vec![String::new()],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 40,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: true,
            trailing_blank_rows: 0,
            query: None,
        };
        let picker = Picker::halfblocks();
        let view = build(&chapter, &palette, &options, &picker, |_| {
            Some(("pic".to_string(), 600, 800))
        });
        let (cols, rows) = view
            .items
            .iter()
            .find_map(|item| match item {
                Item::Image {
                    cols, rows, key, ..
                } => {
                    assert_eq!(key, "pic");
                    Some((*cols, *rows))
                }
                _ => None,
            })
            .unwrap();
        assert!(rows <= 10);
        assert!(cols > 1);
        assert_eq!(rows, 10);
    }

    #[test]
    fn does_not_load_images_when_disabled() {
        let chapter = Chapter {
            href: "ch1.xhtml".into(),
            title: "One".into(),
            blocks: vec![Block::Image {
                src: "pic.png".into(),
                alt: "dot".into(),
            }],
            plain: vec!["dot".into()],
            anchors: HashMap::new(),
            links: Vec::new(),
        };
        let palette = crate::theme::Palette {
            text: ratatui::style::Color::White,
            background: ratatui::style::Color::Black,
            heading: ratatui::style::Color::Yellow,
            accent: ratatui::style::Color::Cyan,
            muted: ratatui::style::Color::Gray,
            search: ratatui::style::Color::Red,
            search_text: ratatui::style::Color::Black,
            status_bg: ratatui::style::Color::Blue,
            status_fg: ratatui::style::Color::White,
            border: ratatui::style::Color::Gray,
            selection: ratatui::style::Color::DarkGray,
        };
        let options = Options {
            width: 40,
            paragraph_spacing: 1,
            image_max_rows: 10,
            images: false,
            trailing_blank_rows: 0,
            query: None,
        };
        let view =
            build(&chapter, &palette, &options, &Picker::halfblocks(), |_| {
                panic!("image metadata must not be read")
            });
        assert!(view
            .items
            .iter()
            .any(|item| matches!(item, Item::Line { plain, .. } if plain.contains("dot"))));
    }
}
