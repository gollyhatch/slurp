//! Command line entry point for slurp.

use std::env;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event,
};
use ratatui::crossterm::execute;
use slurp::app::App;
use slurp::config::Config;
use slurp::library::{Library, LibraryAction};
use slurp::state::{self, Store};
use slurp::theme;

const USAGE: &str = "\
slurp - a terminal EPUB reader

Usage:
  slurp [OPTIONS] [BOOK.epub]

Without a book, slurp opens the recent-books list. With --resume, it opens the
most recently read book instead.

Options:
  -c, --config <FILE>   Use a specific config file
      --state <FILE>    Use a specific reading position file
  -t, --theme <NAME>    Start with a built-in theme
  -w, --width <COLS>    Set the text column width
  -r, --resume          Open the most recently read book
      --list-themes     List the built-in themes
  -h, --help            Show this help
  -V, --version         Show the version

Keys:
  j/k, d/u scroll; Space next paragraph; PgDn/PgUp pages; g/G top/bottom;
  h/l chapter; t contents;
  % jump to percentage; / search; n/N next/previous match;
  Tab next link; Enter follow link; Ctrl+O back after link;
  m add bookmark; ' bookmarks; s reading time; S save settings;
  mouse wheel scrolls; click opens contents and theme entries;
  i toggle images; a auto-scroll; [ / ] speed; c continuous mode;
  p paragraph color bars;
  r pull latest synced position;
  Ctrl+T theme selector;
  +/- text width; ? help; q quit

Recent books: j/k select; Enter open; x remove; c clean missing; q quit

Configuration:
  ~/.config/slurp/config.toml

  theme = \"catppuccin-mocha\"
  text_width = 80
  margin = 2
  top_margin = 1
  bottom_margin = 1
  paragraph_spacing = 1
  image_max_height = 20
  images = true
  auto_scroll_wpm = 200
  reading_wpm = 250
  continuous = false
  paragraph_colors = false

  [sync]
  enabled = false
  server = \"https://sync.crosspointreader.com\"
  username = \"you\"
  password = \"secret\"
  document_id = \"binary\"

  [colors]
  text = \"#cdd6f4\"
  background = \"#1e1e2e\"
  accent = \"#cba6f7\"

Color roles: text, background, heading, accent, muted, search, search_text,
status_bg, status_fg, border, selection.
";

#[derive(Default)]
struct Args {
    book: Option<PathBuf>,
    config: Option<PathBuf>,
    state: Option<PathBuf>,
    theme: Option<String>,
    width: Option<u16>,
    resume: bool,
    list_themes: bool,
    help: bool,
    version: bool,
}

fn main() -> Result<()> {
    let args = parse_args()?;

    if args.help {
        print!("{USAGE}");
        return Ok(());
    }
    if args.version {
        println!("slurp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.list_themes {
        for (id, name) in slurp::theme::builtin_names() {
            println!("{id:<24} {name}");
        }
        return Ok(());
    }

    let mut config = Config::load(args.config.as_deref())?;
    if let Some(theme) = &args.theme {
        config.theme = theme.clone();
    }
    if let Some(width) = args.width {
        config.text_width = width;
    }
    config.validate()?;

    let state_path = args.state.clone().unwrap_or_else(state::default_path);
    if args.resume && args.book.is_some() {
        bail!("--resume cannot be combined with a book");
    }
    let book = if args.resume {
        Some(resume_book(&state_path)?)
    } else {
        args.book.clone()
    };

    let mut terminal =
        ratatui::try_init().context("cannot initialize the terminal")?;
    // Terminals without mouse support keep working with the keyboard.
    let _ = execute!(stdout(), EnableMouseCapture);
    let result = session(&mut terminal, config, state_path, book);
    let _ = execute!(stdout(), DisableMouseCapture);
    let _ = ratatui::try_restore();
    result
}

/// Path of the most recently read book.
fn resume_book(state_path: &Path) -> Result<PathBuf> {
    let store = Store::load(state_path);
    let position = store
        .most_recent()
        .context("no recent books to resume")?;
    let path = PathBuf::from(&position.path);
    if !path.exists() {
        bail!("cannot resume {}: file is missing", path.display());
    }
    Ok(path)
}

/// Run the reader or the recent book list, then return to the shell.
fn session(
    terminal: &mut ratatui::DefaultTerminal,
    config: Config,
    state_path: PathBuf,
    book: Option<PathBuf>,
) -> Result<()> {
    let Some(book_path) = book else {
        return show_library(terminal, config, state_path);
    };
    let mut app = App::new(&book_path, config, detect_picker(), state_path)?;
    run(terminal, &mut app)
}

/// Show the recent book list until the user opens a book or quits.
fn show_library(
    terminal: &mut ratatui::DefaultTerminal,
    config: Config,
    state_path: PathBuf,
) -> Result<()> {
    let (_theme, palette) = theme::load(&config.theme, &config.colors)?;
    let mut library = Library::new(
        Store::load(&state_path),
        palette,
        state_path.clone(),
    );
    loop {
        terminal.draw(|frame| library.draw(frame))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match library.handle_key(key) {
            LibraryAction::Quit => return Ok(()),
            LibraryAction::Open(_) => {
                let Some(path) = library.selected_path().map(str::to_owned)
                else {
                    continue;
                };
                match App::new(
                    Path::new(&path),
                    config.clone(),
                    detect_picker(),
                    state_path.clone(),
                ) {
                    Ok(mut app) => {
                        run(terminal, &mut app)?;
                        return Ok(());
                    }
                    Err(error) => library.set_message(format!("{error:#}")),
                }
            }
            LibraryAction::None => {}
        }
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(app.poll_timeout())? {
            match event::read()? {
                Event::Key(key) => {
                    if app.handle_key(key)? {
                        break;
                    }
                }
                Event::Resize(_, _) => {}
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
        app.tick();
    }

    app.save_position()?;
    app.push_sync()
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut arguments = env::args().skip(1).peekable();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => args.help = true,
            "-V" | "--version" => args.version = true,
            "-r" | "--resume" => args.resume = true,
            "--list-themes" => args.list_themes = true,
            "-c" | "--config" => {
                args.config = Some(PathBuf::from(
                    arguments.next().context("--config needs a file name")?,
                ));
            }
            "--state" => {
                args.state = Some(PathBuf::from(
                    arguments.next().context("--state needs a file name")?,
                ));
            }
            "-t" | "--theme" => {
                args.theme =
                    Some(arguments.next().context("--theme needs a name")?);
            }
            "-w" | "--width" => {
                let value =
                    arguments.next().context("--width needs a number")?;
                args.width = Some(
                    value
                        .parse()
                        .with_context(|| format!("invalid width: {value}"))?,
                );
            }
            "--" => {
                if let Some(book) = arguments.next() {
                    args.book = Some(PathBuf::from(book));
                }
            }
            other if other.starts_with("--config=") => {
                args.config = Some(PathBuf::from(&other["--config=".len()..]));
            }
            other if other.starts_with("--state=") => {
                args.state = Some(PathBuf::from(&other["--state=".len()..]));
            }
            other if other.starts_with("--theme=") => {
                args.theme = Some(other["--theme=".len()..].to_string());
            }
            other if other.starts_with("--width=") => {
                let value = &other["--width=".len()..];
                args.width = Some(
                    value
                        .parse()
                        .with_context(|| format!("invalid width: {value}"))?,
                );
            }
            other if other.starts_with('-') && other != "-" => {
                bail!("unknown option: {other}\n\n{USAGE}");
            }
            other => {
                if args.book.is_some() {
                    bail!("only one book can be opened");
                }
                args.book = Some(PathBuf::from(other));
            }
        }
    }

    Ok(args)
}

/// Pick a graphics protocol from environment variables.
///
/// Querying the terminal reads stdin in a background thread, which swallows
/// the first key press. Environment detection avoids that.
fn detect_picker() -> ratatui_image::picker::Picker {
    use ratatui_image::picker::{Picker, ProtocolType};

    let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
    let program = env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let in_tmux = env::var("TMUX").is_ok_and(|value| !value.is_empty());

    let protocol = if in_tmux {
        // Kitty and iTerm2 escapes need tmux passthrough, which this
        // program does not set up.
        ProtocolType::Halfblocks
    } else if env::var("KITTY_WINDOW_ID").is_ok_and(|value| !value.is_empty())
        || term.contains("kitty")
        || term.contains("ghostty")
        || program == "ghostty"
    {
        ProtocolType::Kitty
    } else if program == "iterm.app"
        || program == "wezterm"
        || term.contains("wezterm")
    {
        ProtocolType::Iterm2
    } else if term.starts_with("foot")
        || term.contains("sixel")
        || term.contains("mlterm")
        || term.contains("yaft")
    {
        ProtocolType::Sixel
    } else {
        ProtocolType::Halfblocks
    };

    if protocol == ProtocolType::Halfblocks {
        return Picker::halfblocks();
    }

    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(cell_size());
    picker.set_protocol_type(protocol);
    picker
}

fn cell_size() -> ratatui_image::FontSize {
    use ratatui_image::FontSize;

    if let Ok(size) = ratatui::crossterm::terminal::window_size() {
        if size.columns > 0
            && size.rows > 0
            && size.width > 0
            && size.height > 0
        {
            return FontSize::new(
                (size.width / size.columns).max(1),
                (size.height / size.rows).max(1),
            );
        }
    }
    FontSize::new(10, 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slurp::state::BookPosition;

    fn position(path: &Path, updated: u64) -> BookPosition {
        BookPosition {
            path: path.to_string_lossy().into_owned(),
            updated,
            ..Default::default()
        }
    }

    #[test]
    fn resumes_the_most_recent_book() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("positions.toml");
        let older = directory.path().join("older.epub");
        let newer = directory.path().join("newer.epub");
        std::fs::write(&older, "").unwrap();
        std::fs::write(&newer, "").unwrap();
        let mut store = Store::default();
        store.books.push(position(&older, 10));
        store.books.push(position(&newer, 20));
        store.save(&state_path).unwrap();

        assert_eq!(resume_book(&state_path).unwrap(), newer);
    }

    #[test]
    fn refuses_to_resume_without_recent_books() {
        let directory = tempfile::tempdir().unwrap();
        let error = resume_book(&directory.path().join("positions.toml"))
            .unwrap_err();
        assert!(
            error.to_string().contains("no recent books"),
            "{error:#}"
        );
    }

    #[test]
    fn refuses_to_resume_a_missing_book() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("positions.toml");
        let mut store = Store::default();
        store
            .books
            .push(position(&directory.path().join("gone.epub"), 10));
        store.save(&state_path).unwrap();
        let error = resume_book(&state_path).unwrap_err();
        assert!(error.to_string().contains("missing"), "{error:#}");
    }
}
