//! Test fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A temporary EPUB directory.
pub struct TestBook {
    _dir: TempDir,
    pub path: PathBuf,
}

/// Create a small unpacked EPUB with two chapters and one image.
pub fn sample_book() -> TestBook {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    write(root, "mimetype", "application/epub+zip");
    write(
        root,
        "META-INF/container.xml",
        r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    );
    write(
        root,
        "OEBPS/content.opf",
        r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:slurp-test</dc:identifier>
    <dc:title>Test Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="img" href="img/pic.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#,
    );
    write(
        root,
        "OEBPS/nav.xhtml",
        r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <head><title>Contents</title></head>
  <body>
    <nav epub:type="toc">
      <ol>
        <li><a href="ch1.xhtml">First Chapter</a></li>
        <li><a href="ch1.xhtml#part-two">Part Two</a></li>
        <li><a href="ch2.xhtml">Second Chapter</a></li>
      </ol>
    </nav>
  </body>
</html>"#,
    );
    write(
        root,
        "OEBPS/ch1.xhtml",
        r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter One</title></head>
  <body>
    <h1>Chapter One</h1>
    <p>Alice was beginning to get very tired of sitting by her sister.</p>
    <p>The <strong>alpha</strong> and <em>beta</em> and <code>gamma</code> words.</p>
    <ul>
      <li>delta item</li>
      <li>epsilon item</li>
    </ul>
    <blockquote>A quoted line about curious things.</blockquote>
    <p>First line<br/>second line</p>
    <h2 id="part-two">Part Two</h2>
    <p>The second part starts here.</p>
    <img src="img/pic.png" alt="A small dot"/>
  </body>
</html>"#,
    );
    write(
        root,
        "OEBPS/ch2.xhtml",
        r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter Two</title></head>
  <body>
    <h1>Chapter Two</h1>
    <p>Zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma.</p>
    <p>Second paragraph with the word theta again.</p>
    <p>Paragraph 1 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 2 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 3 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 4 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 5 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 6 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 7 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 8 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 9 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 10 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 11 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 12 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 13 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 14 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 15 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 16 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 17 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 18 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 19 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 20 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 21 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 22 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 23 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 24 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
    <p>Paragraph 25 has enough words to wrap across several terminal lines and to make the chapter long enough to scroll. Theta appears only in some paragraphs.</p>
  </body>
</html>"#,
    );
    write_bytes(root, "OEBPS/img/pic.png", PNG_1X1);

    TestBook {
        path: root.to_path_buf(),
        _dir: dir,
    }
}

/// A small EPUB whose navigation document has no entries.
pub fn sample_book_without_toc() -> TestBook {
    let book = sample_book();
    write(
        &book.path,
        "OEBPS/nav.xhtml",
        r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <head><title>Contents</title></head>
  <body>
    <nav epub:type="toc"><ol/></nav>
  </body>
</html>"#,
    );
    book
}

fn write(root: &Path, relative: &str, content: &str) {
    write_bytes(root, relative, content.as_bytes());
}

fn write_bytes(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture dir");
    }
    fs::write(path, content).expect("write fixture");
}

/// A 1x1 PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
    0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
    0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
