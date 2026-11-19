//! Turn XHTML chapter content into styled blocks.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// Inline flags for a run of text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineFlags {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Set when the run is part of a link.
    pub link: bool,
}

/// A run of text with uniform inline flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub flags: InlineFlags,
}

impl Inline {
    fn new(text: String, flags: InlineFlags) -> Self {
        Self { text, flags }
    }
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    /// A soft line break inside a paragraph (`<br/>`).
    Line(Vec<Inline>),
    Quote(Vec<Inline>),
    Pre(Vec<Inline>),
    ListItem {
        marker: String,
        depth: usize,
        content: Vec<Inline>,
    },
    Rule,
    Image {
        src: String,
        alt: String,
    },
}

impl Block {
    /// Plain text of the block, without markup.
    pub fn plain_text(&self) -> String {
        let content = match self {
            Block::Heading { content, .. }
            | Block::Paragraph(content)
            | Block::Line(content)
            | Block::Quote(content)
            | Block::Pre(content)
            | Block::ListItem { content, .. } => content,
            Block::Rule => return String::new(),
            Block::Image { alt, .. } => return alt.clone(),
        };
        plain_text(content)
    }
}

/// Join inline runs into plain text.
pub fn plain_text(inlines: &[Inline]) -> String {
    inlines.iter().map(|inline| inline.text.as_str()).collect()
}

/// Result of parsing one XHTML document.
#[derive(Debug, Clone, Default)]
pub struct ParsedHtml {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    /// Text of the headings, in document order.
    pub headings: Vec<String>,
    /// Element ids and the block index they point at.
    pub anchors: Vec<(String, usize)>,
    /// Links in document order.
    pub links: Vec<Link>,
}

/// A link found in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub href: String,
    pub label: String,
    /// Index of the block that contains the link text.
    pub block: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum BlockKind {
    #[default]
    Paragraph,
    Heading(u8),
    Quote,
    Pre,
    ListItem,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Ordered,
}

#[derive(Default)]
struct Parser {
    blocks: Vec<Block>,
    current: Vec<Inline>,
    kind: BlockKind,
    bold: usize,
    italic: usize,
    code: usize,
    pre: bool,
    skip: usize,
    in_head: bool,
    in_title: bool,
    title: String,
    headings: Vec<String>,
    lists: Vec<ListKind>,
    counters: Vec<u32>,
    markers: Vec<String>,
    anchors: Vec<(String, usize)>,
    pending_space: bool,
    link_depth: usize,
    link_href: Option<String>,
    link_label: String,
    link_block: usize,
    links: Vec<Link>,
}

impl Parser {
    fn start(&mut self, name: &str, attrs: &HashMap<String, String>) {
        if self.skip == 0 && !self.in_head {
            if let Some(id) = attrs.get("id").or_else(|| attrs.get("name")) {
                if !id.is_empty()
                    && !self.anchors.iter().any(|(known, _)| known == id)
                {
                    self.anchors.push((id.clone(), self.blocks.len()));
                }
            }
        }

        match name {
            "script" | "style" | "rt" | "rp" => self.skip += 1,
            "head" => self.in_head = true,
            "title" => self.in_title = true,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.kind = BlockKind::Heading(name[1..].parse().unwrap_or(1));
            }
            "p" | "div" | "section" | "article" | "header" | "footer"
            | "main" | "aside" | "body" | "html" | "figure" | "figcaption"
            | "td" | "th" | "caption" => {
                self.flush();
                self.kind = if self.markers.is_empty() {
                    BlockKind::Paragraph
                } else {
                    BlockKind::ListItem
                };
            }
            "blockquote" => {
                self.flush();
                self.kind = BlockKind::Quote;
            }
            "pre" => {
                self.flush();
                self.kind = BlockKind::Pre;
                self.pre = true;
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.push(if name == "ol" {
                    ListKind::Ordered
                } else {
                    ListKind::Bullet
                });
                self.counters.push(0);
                self.kind = BlockKind::Paragraph;
            }
            "li" => {
                self.flush();
                let marker = match self.lists.last() {
                    Some(ListKind::Ordered) => {
                        let counter =
                            self.counters.last_mut().expect("list counter");
                        *counter += 1;
                        format!("{}.", counter)
                    }
                    _ => "\u{2022}".to_string(),
                };
                self.markers.push(marker);
                self.kind = BlockKind::ListItem;
            }
            "br" => {
                self.flush();
                self.kind = BlockKind::Line;
            }
            "hr" => {
                self.flush();
                self.blocks.push(Block::Rule);
            }
            "img" => {
                if let Some(src) = attrs.get("src") {
                    let alt = attrs.get("alt").cloned().unwrap_or_default();
                    self.flush();
                    self.blocks.push(Block::Image {
                        src: src.clone(),
                        alt,
                    });
                }
            }
            "image" => {
                if let Some(src) =
                    attrs.get("href").or_else(|| attrs.get("src"))
                {
                    let alt = attrs
                        .get("alt")
                        .or_else(|| attrs.get("title"))
                        .cloned()
                        .unwrap_or_default();
                    self.flush();
                    self.blocks.push(Block::Image {
                        src: src.clone(),
                        alt,
                    });
                }
            }
            "a" => {
                if self.link_depth == 0 {
                    if let Some(href) =
                        attrs.get("href").filter(|href| !href.is_empty())
                    {
                        self.link_href = Some(href.clone());
                        self.link_label.clear();
                        self.link_block = self.blocks.len();
                        self.link_depth = 1;
                    }
                } else {
                    self.link_depth += 1;
                }
            }
            "b" | "strong" => self.bold += 1,
            "i" | "em" => self.italic += 1,
            "code" | "tt" | "kbd" | "samp" => self.code += 1,
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "script" | "style" | "rt" | "rp" => {
                self.skip = self.skip.saturating_sub(1)
            }
            "head" => self.in_head = false,
            "title" => {
                self.in_title = false;
                self.title = collapse_whitespace(&self.title);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div"
            | "section" | "article" | "header" | "footer" | "main"
            | "aside" | "body" | "html" | "figure" | "figcaption" | "td"
            | "th" | "caption" | "blockquote" => {
                self.flush();
                self.kind = if self.markers.is_empty() {
                    BlockKind::Paragraph
                } else {
                    BlockKind::ListItem
                };
            }
            "pre" => {
                self.flush();
                self.pre = false;
                self.kind = BlockKind::Paragraph;
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
                self.counters.pop();
                self.kind = if self.markers.is_empty() {
                    BlockKind::Paragraph
                } else {
                    BlockKind::ListItem
                };
            }
            "li" => {
                self.flush();
                self.markers.pop();
                self.kind = if self.markers.is_empty() {
                    BlockKind::Paragraph
                } else {
                    BlockKind::ListItem
                };
            }
            "a" => self.finish_link(),
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" => self.italic = self.italic.saturating_sub(1),
            "code" | "tt" | "kbd" | "samp" => {
                self.code = self.code.saturating_sub(1)
            }
            _ => {}
        }
    }

    fn text(&mut self, raw: &str) {
        if self.skip > 0 {
            return;
        }
        if self.in_title {
            self.title.push_str(raw);
            return;
        }
        if self.in_head {
            return;
        }

        let mut out = String::new();
        let mut previous_space = self.pending_space || self.current.is_empty();
        for ch in raw.chars() {
            let ch = if ch == '\u{a0}' || ch == '\u{200b}' {
                ' '
            } else {
                ch
            };
            if self.pre {
                out.push(ch);
            } else if ch.is_whitespace() {
                if !previous_space {
                    out.push(' ');
                    previous_space = true;
                }
            } else {
                out.push(ch);
                previous_space = false;
            }
        }
        self.pending_space = previous_space;
        if !out.is_empty() {
            if self.link_depth > 0 {
                self.link_label.push_str(&out);
            }
            self.push_text(&out);
        }
    }

    /// Close the innermost open link.
    fn finish_link(&mut self) {
        if self.link_depth == 0 {
            return;
        }
        self.link_depth -= 1;
        if self.link_depth != 0 {
            return;
        }
        if let Some(href) = self.link_href.take() {
            self.links.push(Link {
                href,
                label: collapse_whitespace(&self.link_label),
                block: self.link_block,
            });
        }
        self.link_label.clear();
    }

    fn reference(&mut self, reference: &str) {
        if self.skip > 0 {
            return;
        }
        let resolved = if let Some(rest) = reference.strip_prefix('#') {
            let value = rest.strip_prefix(['x', 'X']).unwrap_or(rest);
            let radix = if rest.starts_with(['x', 'X']) { 16 } else { 10 };
            u32::from_str_radix(value, radix)
                .ok()
                .and_then(char::from_u32)
                .map(String::from)
        } else {
            quick_xml::escape::resolve_predefined_entity(reference)
                .map(String::from)
        };
        if let Some(text) = resolved {
            self.text(&text);
        }
    }

    fn push_text(&mut self, text: &str) {
        let flags = InlineFlags {
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: self.code > 0,
            link: self.link_depth > 0,
        };
        if let Some(last) = self.current.last_mut() {
            if last.flags == flags {
                last.text.push_str(text);
                return;
            }
        }
        self.current.push(Inline::new(text.to_string(), flags));
    }

    fn flush(&mut self) {
        if !matches!(self.kind, BlockKind::Pre) {
            trim_inlines(&mut self.current);
        }
        self.pending_space = false;
        if self.current.is_empty() {
            return;
        }

        let content = std::mem::take(&mut self.current);
        match self.kind.clone() {
            BlockKind::Paragraph => self.blocks.push(Block::Paragraph(content)),
            BlockKind::Heading(level) => {
                self.headings.push(plain_text(&content));
                self.blocks.push(Block::Heading { level, content });
            }
            BlockKind::Quote => self.blocks.push(Block::Quote(content)),
            BlockKind::Pre => self.blocks.push(Block::Pre(content)),
            BlockKind::Line => self.blocks.push(Block::Line(content)),
            BlockKind::ListItem => {
                let marker = self
                    .markers
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "\u{2022}".into());
                let depth = self.markers.len();
                self.blocks.push(Block::ListItem {
                    marker,
                    depth,
                    content,
                });
            }
        }
    }

    fn finish(mut self) -> ParsedHtml {
        self.flush();
        self.finish_link();
        let title = if self.title.is_empty() {
            None
        } else {
            Some(self.title)
        };
        ParsedHtml {
            title,
            blocks: self.blocks,
            headings: self.headings,
            anchors: self.anchors,
            links: self.links,
        }
    }
}

/// Parse an XHTML document.
///
/// The parser is forgiving: malformed markup stops the parse at the error but
/// keeps all blocks seen so far.
pub fn parse(xhtml: &str) -> ParsedHtml {
    let mut reader = Reader::from_str(xhtml);
    reader.config_mut().allow_dangling_amp = true;
    reader.config_mut().allow_unmatched_ends = true;
    reader.config_mut().check_end_names = false;

    let mut parser = Parser::default();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                parser.start(&local_name(&event), &attributes(&event));
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(&event);
                let attrs = attributes(&event);
                parser.start(&name, &attrs);
                parser.end(&name);
            }
            Ok(Event::End(event)) => {
                parser.end(local_name_bytes(event.name().as_ref()));
            }
            Ok(Event::Text(text)) => {
                parser.text(&text.xml10_content());
            }
            Ok(Event::CData(text)) => {
                parser.text(text.as_ref());
            }
            Ok(Event::GeneralRef(reference)) => {
                parser.reference(reference.as_ref());
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    parser.finish()
}

fn attributes(event: &BytesStart<'_>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for attr in event.attributes().flatten() {
        let key = attr.key.local_name().as_ref().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| attr.value.as_ref().to_string());
        map.insert(key, value);
    }
    map
}

fn local_name(event: &BytesStart<'_>) -> String {
    local_name_bytes(event.name().as_ref()).to_string()
}

fn local_name_bytes(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, local)| local)
}

fn trim_inlines(inlines: &mut Vec<Inline>) {
    while let Some(first) = inlines.first_mut() {
        let trimmed = first.text.trim_start();
        if trimmed.len() == first.text.len() {
            break;
        }
        first.text = trimmed.to_string();
        if first.text.is_empty() {
            inlines.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = inlines.last_mut() {
        let trimmed = last.text.trim_end();
        if trimmed.len() == last.text.len() {
            break;
        }
        last.text = trimmed.to_string();
        if last.text.is_empty() {
            inlines.pop();
        } else {
            break;
        }
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"<?xml version="1.0"?>
        <html xmlns="http://www.w3.org/1999/xhtml">
          <head><title>Chapter One &amp; More</title></head>
          <body>
            <h1>Chapter One</h1>
            <p>Hello <strong>bold</strong> and <em>italic</em> &amp; <code>code</code>.</p>
            <p>Second&nbsp;paragraph with an entity.</p>
            <ul>
              <li>first item</li>
              <li>second item</li>
            </ul>
            <ol>
              <li>one</li>
              <li>two</li>
            </ol>
            <blockquote>quoted text</blockquote>
            <p>line one<br/>line two</p>
            <pre>  preformatted
  text</pre>
            <img src="../img/pic.png" alt="A picture"/>
            <hr/>
          </body>
        </html>"#
    }

    #[test]
    fn parses_structure() {
        let parsed = parse(sample());
        assert_eq!(parsed.title.as_deref(), Some("Chapter One & More"));
        assert_eq!(parsed.headings, vec!["Chapter One"]);

        assert!(matches!(parsed.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(parsed.blocks[1], Block::Paragraph(_)));
        let paragraph = parsed.blocks[1].plain_text();
        assert_eq!(paragraph, "Hello bold and italic & code.");

        let Some(Block::Paragraph(content)) = parsed.blocks.get(1) else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| inline.flags.bold));
        assert!(content.iter().any(|inline| inline.flags.italic));
        assert!(content.iter().any(|inline| inline.flags.code));
    }

    #[test]
    fn parses_lists_and_pre() {
        let parsed = parse(sample());
        let list_items: Vec<_> = parsed
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::ListItem { marker, .. } => Some(marker.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(list_items, vec!["\u{2022}", "\u{2022}", "1.", "2."]);
        assert!(parsed
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Pre(_))));
        assert_eq!(
            parsed
                .blocks
                .iter()
                .find_map(|block| match block {
                    Block::Pre(content) => Some(plain_text(content)),
                    _ => None,
                })
                .unwrap(),
            "  preformatted\n  text"
        );
    }

    #[test]
    fn parses_breaks_and_images() {
        let parsed = parse(sample());
        let line_indexes: Vec<_> = parsed
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| matches!(block, Block::Line(_)))
            .map(|(index, _)| index)
            .collect();
        assert!(!line_indexes.is_empty());
        assert!(parsed.blocks.iter().any(|block| matches!(
            block,
            Block::Image { src, alt } if src == "../img/pic.png" && alt == "A picture"
        )));
        assert!(parsed
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Rule)));
    }

    #[test]
    fn records_anchors() {
        let html = r#"<html><body>
            <h2 id="chapter-1">One</h2>
            <p>text</p>
            <a id="chapter-2"></a>
            <p>more</p>
        </body></html>"#;
        let parsed = parse(html);
        assert_eq!(parsed.anchors.len(), 2);
        assert_eq!(parsed.anchors[0].0, "chapter-1");
        assert_eq!(parsed.anchors[0].1, 0);
        assert_eq!(parsed.anchors[1].0, "chapter-2");
        assert_eq!(parsed.anchors[1].1, 2);
    }

    #[test]
    fn handles_svg_image() {
        let html = r#"<html><body><svg><image xlink:href="cover.jpg"/></svg></body></html>"#;
        let parsed = parse(html);
        assert!(matches!(
            parsed.blocks.first(),
            Some(Block::Image { src, .. }) if src == "cover.jpg"
        ));
    }

    #[test]
    fn tolerates_bad_markup() {
        let html = "<html><body><p>good text<div>more";
        let parsed = parse(html);
        assert!(!parsed.blocks.is_empty());
    }
}
