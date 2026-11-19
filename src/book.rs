//! EPUB loading and resource access.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use percent_encoding::percent_decode_str;
use rbook::epub::Epub;

use crate::html::{self, Block};

/// One readable document from the EPUB spine.
pub struct Chapter {
    /// Manifest href of the document.
    pub href: String,
    /// Display title.
    pub title: String,
    /// Parsed content.
    pub blocks: Vec<Block>,
    /// Plain text of each block, used for full-text search.
    pub plain: Vec<String>,
}

/// An opened EPUB.
pub struct Book {
    epub: Epub,
    pub path: PathBuf,
    pub title: String,
    pub author: Option<String>,
    pub chapters: Vec<Chapter>,
    image_hrefs: HashMap<String, String>,
}

impl Book {
    /// Open an EPUB file or an unpacked EPUB directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let epub = Epub::open(&path)
            .with_context(|| format!("cannot open EPUB {}", path.display()))?;

        let title = epub
            .metadata()
            .title()
            .map(|entry| entry.value().trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled".to_string())
            });

        let author = epub
            .metadata()
            .creators()
            .next()
            .map(|entry| entry.value().trim().to_string())
            .filter(|value| !value.is_empty());

        let mut toc_anchors: HashMap<String, Vec<TocAnchor>> = HashMap::new();
        for root in epub.toc().iter() {
            collect_toc_anchors(&root, &mut toc_anchors);
            for entry in root.flatten() {
                collect_toc_anchors(&entry, &mut toc_anchors);
            }
        }

        let mut image_hrefs = HashMap::new();
        for entry in epub.manifest().images() {
            let href = entry.href().as_ref().to_string();
            image_hrefs.insert(normalize_path(&href), href);
        }

        let mut chapters = Vec::new();
        let mut reader = epub.reader();
        loop {
            let content = match reader.read_next() {
                Some(Ok(content)) => content,
                Some(Err(_)) => continue,
                None => break,
            };

            let entry = content.manifest_entry();
            if !matches!(entry.kind().subtype(), "xhtml" | "html") {
                continue;
            }
            if entry.properties().iter().any(|property| property == "nav") {
                continue;
            }

            let href = entry.href().as_ref().to_string();
            let parsed = html::parse(content.content());
            if parsed.blocks.is_empty() {
                continue;
            }

            let key = normalize_path(&href);
            let toc = toc_anchors.get(&key).map(Vec::as_slice);

            for section in split_sections(parsed.blocks, &parsed.anchors, toc) {
                let title = section
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| parsed.title.clone())
                    .unwrap_or_else(|| {
                        format!("Chapter {}", chapters.len() + 1)
                    });

                let plain =
                    section.blocks.iter().map(Block::plain_text).collect();
                chapters.push(Chapter {
                    href: href.clone(),
                    title: collapse_spaces(&title),
                    blocks: section.blocks,
                    plain,
                });
            }
        }

        if chapters.is_empty() {
            chapters.push(Chapter {
                href: String::new(),
                title: "Empty book".to_string(),
                blocks: Vec::new(),
                plain: Vec::new(),
            });
        }

        Ok(Self {
            epub,
            path,
            title: collapse_spaces(&title),
            author,
            chapters,
            image_hrefs,
        })
    }

    /// Resolve an image source inside a chapter to a manifest key.
    pub fn image_key(&self, chapter_href: &str, src: &str) -> Option<String> {
        let src = src.trim();
        if src.is_empty() || src.starts_with("data:") || src.contains("://") {
            return None;
        }
        let src = src.split('#').next().unwrap_or(src);
        let src = percent_decode_str(src).decode_utf8_lossy();
        let base = percent_decode_str(chapter_href).decode_utf8_lossy();

        let mut parts: Vec<&str> = base
            .trim_start_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        parts.pop();

        let mut resolved: Vec<String> =
            parts.iter().map(|part| (*part).to_string()).collect();
        for segment in src.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    resolved.pop();
                }
                other => resolved.push(other.to_string()),
            }
        }
        Some(resolved.join("/"))
    }

    /// Read the bytes of an image by its resolved manifest key.
    pub fn image_bytes_by_key(&self, key: &str) -> Option<Vec<u8>> {
        let href = self.image_hrefs.get(key)?;
        self.epub
            .manifest()
            .by_href(href)
            .and_then(|entry| entry.read_bytes().ok())
    }

    /// Read the bytes of an image referenced from a chapter.
    pub fn image_bytes(
        &self,
        chapter_href: &str,
        src: &str,
    ) -> Option<Vec<u8>> {
        let key = self.image_key(chapter_href, src)?;
        let href = self.image_hrefs.get(&key)?;
        self.epub
            .manifest()
            .by_href(href)
            .and_then(|entry| entry.read_bytes().ok())
    }
}

#[derive(Debug, Clone)]
struct TocAnchor {
    fragment: Option<String>,
    label: String,
}

struct Section {
    title: Option<String>,
    blocks: Vec<Block>,
}

/// Split one XHTML document into sections.
///
/// Anchors from the table of contents take priority. Without usable anchors,
/// level 1 and 2 headings mark chapter starts.
fn split_sections(
    blocks: Vec<Block>,
    anchors: &[(String, usize)],
    toc: Option<&[TocAnchor]>,
) -> Vec<Section> {
    let mut cuts = Vec::new();
    if let Some(toc) = toc {
        for entry in toc {
            let index = match &entry.fragment {
                Some(fragment) => anchors
                    .iter()
                    .find(|(id, _)| id == fragment)
                    .map(|(_, index)| *index),
                None => Some(0),
            };
            if let Some(index) = index {
                if index < blocks.len() && !entry.label.is_empty() {
                    cuts.push((index, entry.label.clone()));
                }
            }
        }
    }
    cuts.sort_by_key(|(index, _)| *index);
    cuts.dedup_by_key(|(index, _)| *index);

    let use_cuts =
        cuts.len() > 1 || cuts.first().is_some_and(|(index, _)| *index > 0);
    if !use_cuts {
        let mut sections = heading_sections(blocks);
        if sections.len() == 1 && sections[0].title.is_none() {
            if let Some(label) = cuts.first() {
                sections[0].title = Some(label.1.clone());
            }
        }
        return sections;
    }

    if cuts.first().is_none_or(|(index, _)| *index > 0) {
        cuts.insert(0, (0, String::new()));
    }

    let mut sections = Vec::new();
    for (number, (start, label)) in cuts.iter().enumerate() {
        let end = cuts
            .get(number + 1)
            .map(|(index, _)| *index)
            .unwrap_or(blocks.len());
        if start >= &end {
            continue;
        }
        sections.push(Section {
            title: (!label.is_empty()).then(|| label.clone()),
            blocks: blocks[*start..end].to_vec(),
        });
    }
    sections
}

fn heading_sections(blocks: Vec<Block>) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current = Vec::new();
    let mut title: Option<String> = None;
    let mut has_text = false;

    for block in blocks {
        let heading = match &block {
            Block::Heading { level, content } if *level <= 2 => {
                Some(html::plain_text(content))
            }
            _ => None,
        };

        if heading.is_some() && has_text {
            sections.push(Section {
                title: title.take(),
                blocks: std::mem::take(&mut current),
            });
            has_text = false;
        }
        if let Some(heading) = heading {
            title = Some(heading);
        }
        if !matches!(
            block,
            Block::Heading { .. } | Block::Rule | Block::Image { .. }
        ) {
            has_text = true;
        }
        current.push(block);
    }

    if !current.is_empty() {
        sections.push(Section {
            title,
            blocks: current,
        });
    }
    sections
}

fn collect_toc_anchors(
    entry: &rbook::epub::toc::EpubTocEntry<'_>,
    anchors: &mut HashMap<String, Vec<TocAnchor>>,
) {
    let Some(href) = entry.href() else {
        return;
    };
    let raw = href.as_ref();
    let (path, fragment) = match raw.split_once('#') {
        Some((path, fragment)) => (
            path,
            Some(
                percent_decode_str(fragment)
                    .decode_utf8_lossy()
                    .into_owned(),
            ),
        ),
        None => (raw, None),
    };
    let path = path.split('?').next().unwrap_or(path);
    let key = normalize_path(path);
    let label = entry.label().trim().to_string();
    anchors
        .entry(key)
        .or_default()
        .push(TocAnchor { fragment, label });
}

/// Normalize a manifest href to a lookup key: decoded, without a fragment or
/// leading slash.
pub fn normalize_path(href: &str) -> String {
    let path = href.split('#').next().unwrap_or(href);
    let path = path.split('?').next().unwrap_or(path);
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    decoded.trim_start_matches('/').to_string()
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Find an EPUB file next to a path, used by the command line fallback.
pub fn looks_like_epub(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.eq_ignore_ascii_case("epub"))
        .unwrap_or(false)
        || fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_sample_book() {
        let fixture = crate::testutil::sample_book();
        let book = Book::open(&fixture.path).unwrap();

        assert_eq!(book.title, "Test Book");
        assert_eq!(book.author.as_deref(), Some("Test Author"));
        assert_eq!(book.chapters.len(), 3);
        assert_eq!(book.chapters[0].title, "First Chapter");
        assert_eq!(book.chapters[1].title, "Part Two");
        assert_eq!(book.chapters[2].title, "Chapter Two");
        assert!(book.chapters[0]
            .plain
            .iter()
            .any(|text| text.contains("Alice")));
        assert!(book.chapters[2]
            .plain
            .iter()
            .any(|text| text.contains("theta")));
    }

    #[test]
    fn reads_chapter_images() {
        let fixture = crate::testutil::sample_book();
        let book = Book::open(&fixture.path).unwrap();
        let chapter = book
            .chapters
            .iter()
            .find(|chapter| {
                chapter
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::Image { .. }))
            })
            .unwrap();
        let bytes = book.image_bytes(&chapter.href, "img/pic.png").unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    fn heading(level: u8, text: &str) -> Block {
        Block::Heading {
            level,
            content: vec![crate::html::Inline {
                text: text.to_string(),
                flags: Default::default(),
            }],
        }
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(vec![crate::html::Inline {
            text: text.to_string(),
            flags: Default::default(),
        }])
    }

    #[test]
    fn splits_documents_at_headings() {
        let sections = split_sections(
            vec![
                heading(1, "Book Title"),
                paragraph("front matter"),
                heading(2, "Chapter One"),
                paragraph("one"),
                heading(2, "Chapter Two"),
                paragraph("two"),
            ],
            &[],
            None,
        );
        let titles: Vec<_> = sections
            .iter()
            .map(|section| section.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(titles, vec!["Book Title", "Chapter One", "Chapter Two"]);
        assert_eq!(sections.len(), 3);
    }

    #[test]
    fn keeps_consecutive_headings_together() {
        let sections = split_sections(
            vec![
                heading(1, "Book Title"),
                heading(2, "Chapter One"),
                paragraph("one"),
            ],
            &[],
            None,
        );
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title.as_deref(), Some("Chapter One"));
    }

    #[test]
    fn splits_documents_at_toc_anchors() {
        let anchors = vec![("a".to_string(), 0usize), ("b".to_string(), 2)];
        let toc = vec![
            TocAnchor {
                fragment: Some("a".into()),
                label: "One".into(),
            },
            TocAnchor {
                fragment: Some("b".into()),
                label: "Two".into(),
            },
        ];
        let sections = split_sections(
            vec![
                paragraph("one"),
                paragraph("one more"),
                paragraph("two"),
                paragraph("two more"),
            ],
            &anchors,
            Some(&toc),
        );
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title.as_deref(), Some("One"));
        assert_eq!(sections[0].blocks.len(), 2);
        assert_eq!(sections[1].title.as_deref(), Some("Two"));
        assert_eq!(sections[1].blocks.len(), 2);
    }

    #[test]
    fn keeps_front_matter_before_first_anchor() {
        let anchors = vec![("chapter".to_string(), 2usize)];
        let toc = vec![TocAnchor {
            fragment: Some("chapter".into()),
            label: "Chapter".into(),
        }];
        let sections = split_sections(
            vec![
                paragraph("title page"),
                paragraph("copyright"),
                paragraph("chapter text"),
            ],
            &anchors,
            Some(&toc),
        );
        assert_eq!(sections.len(), 2);
        assert!(sections[0].title.is_none());
        assert_eq!(sections[1].title.as_deref(), Some("Chapter"));
    }

    #[test]
    fn does_not_split_at_deep_headings() {
        let sections = split_sections(
            vec![
                heading(1, "Chapter One"),
                paragraph("one"),
                heading(3, "A Subsection"),
                paragraph("two"),
            ],
            &[],
            None,
        );
        assert_eq!(sections.len(), 1);
    }

    #[test]
    fn resolves_relative_paths() {
        let fixture = crate::testutil::sample_book();
        let book = Book::open(&fixture.path).unwrap();
        assert_eq!(
            book.image_key("OEBPS/text/ch1.xhtml", "../img/pic.png"),
            Some("OEBPS/img/pic.png".to_string())
        );
        assert_eq!(
            book.image_key("OEBPS/ch1.xhtml", "img/pic.png"),
            Some("OEBPS/img/pic.png".to_string())
        );
        assert_eq!(book.image_key("OEBPS/ch1.xhtml", "https://x/y.png"), None);
        assert_eq!(
            book.image_key("OEBPS/ch1.xhtml", "data:image/png;base64,AA"),
            None
        );
    }
}
