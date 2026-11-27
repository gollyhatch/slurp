# slurp

A terminal EPUB reader with vim-like keys, full-text search, percentage
jumps, inline images, and per-book reading positions.

```
slurp book.epub
```

## Screenshots

All screenshots use the built-in `ayu-dark` theme.

![Reading view](screenshots/reader.png)

*Reading view: reflowed text, emphasis, quotes, and the status line.*

![Paragraph colors](screenshots/paragraph-colors.png)

*Optional paragraph color bars (`p`): five theme colors cycle through the
text.*

![Paragraph reading mode](screenshots/paragraph-mode.png)

*Optional by-paragraph mode (`P`): the selected paragraph has a primary-accent
bar and the other body text is muted.*

![Contents popup](screenshots/contents.png)

*Contents popup with fuzzy filtering.*

![Search](screenshots/search.png)

*Full-text search with highlighted matches and a match counter.*

![Bookmarks](screenshots/bookmarks.png)

*Named bookmarks with chapter and chapter progress.*

![Reading stats](screenshots/stats.png)

*Reading time and words left for the chapter and the book.*

![Recent books](screenshots/library.png)

*Recent books with chapter and whole-book progress.*

## Features

- Reflows EPUB text to any terminal width. Set a maximum text width in the
  config file or change it while reading with `+` and `-`.
- Vim-like scrolling and chapter navigation. Chapters follow the EPUB
  table of contents, including several chapters inside one file.
- Full-text search across all chapters with `/`, and match highlighting.
- Follow internal links and footnotes with `Tab`, `Enter`, and `Ctrl+O` to go
  back.
- Named bookmarks per book with `m` and `'`, including a jump back to the
  saved place.
- Jump anywhere in the book with `%` and a percentage.
- Auto-scroll at a configurable speed in words per minute. It stops at
  popups and moves to the next chapter when the screen ends.
- Optional continuous mode that scrolls through all chapters as one stream,
  with no chapter switch needed. Two blank rows separate chapters.
- Optional thin color lines left of each paragraph. Five theme colors cycle
  through the text, so a paragraph is easy to find again after scrolling.
- Optional by-paragraph mode that marks the current paragraph with a
  primary-accent bar and mutes the other body paragraphs. Space or `j` selects
  the next one, and the page turns only after the last paragraph on the screen.
- The `t` popup uses the EPUB table of contents with fuzzy filtering. If the
  book has none, the popup says so.
- Inline images with Sixel, Kitty, iTerm2, or Unicode halfblocks.
- Remembers the chapter, scroll position, and selected paragraph for each book
  and returns to the same place on the next start.
- Shows the time left for the chapter and the book, in the status line and in
  a popup on `s`.
- Runs without a book and opens a recent-books list with chapter and
  whole-book progress. `x` removes the selected book and its saved position,
  `c` removes every entry whose file is missing. `--resume` skips the list and
  opens the most recently read book.
- Optional KOSync and CrossPoint Sync progress synchronization across
  devices.
- Saves theme, width, and mode changes back to the config file with `S`,
  keeping comments and unrelated keys.
- Uses the 39 built-in [opaline](https://crates.io/crates/opaline) themes.
  Press `Ctrl+T` to search the themes and preview them live.
  Every color role can be overridden in the config file.
- Images can be turned off from the config file or with `i`.
- The mouse wheel scrolls, and a click opens an entry in a popup.

## Build

```sh
cargo build --release
```

The binary is `target/release/slurp`.

## Keys

| Key | Action |
| --- | --- |
| `j` / `k`, `Down` / `Up` | Scroll one line, or select the next/previous paragraph in paragraph mode |
| `d` / `u` | Scroll half a page |
| `Space` | Start of the next paragraph, or select the next paragraph in paragraph mode |
| `PageDown` / `PageUp`, `b` | Scroll one page (`b` goes up) |
| `g` / `G`, `Home` / `End` | Start or end of the chapter |
| `h` / `l`, `Left` / `Right` | Previous or next chapter |
| `t` | Fuzzy-searchable table of contents from the EPUB |
| `%` | Jump to a percentage of the book |
| `/` | Search all chapters |
| `Enter` | Run the search, apply the percentage, follow the selected link, or open the selected chapter |
| `Tab` | Select the next link in the chapter |
| `Ctrl+O` | Go back after following a link |
| `n` / `N` | Next or previous match |
| `m` | Add a named bookmark |
| `'` | Open the bookmark list (`Enter` jumps, `x` deletes) |
| `s` | Reading stats for the chapter and the book |
| `S` | Save the changed settings to the config file |
| `r` | Fetch the newest synced position and jump to it |
| `i` | Toggle images |
| `a` | Toggle auto-scroll |
| `c` | Toggle continuous mode |
| `p` | Toggle paragraph color bars |
| `P` | Toggle paragraph reading mode |
| `z` | Scroll so the selected paragraph is at the top |
| `[` / `]` | Slower or faster auto-scroll (25 words per minute steps) |
| `Ctrl+T` | Theme selector with search and live preview |
| `+` / `-` | Wider or narrower text column |
| `?` | Show the key help |
| `Esc` | Cancel a prompt, close a popup, or clear the search |
| Mouse wheel, click | Scroll the page, and activate entries in popups |
| `q`, `Ctrl-C` | Save the position and quit |

## Configuration

The config file is `$XDG_CONFIG_HOME/slurp/config.toml`, usually
`~/.config/slurp/config.toml`. Every key is optional.

```toml
# A built-in theme name, or a path to a theme TOML file.
theme = "catppuccin-mocha"

# Maximum width of the text column, in terminal cells.
text_width = 80

# Minimum empty cells on each side of the text column.
margin = 2

# Empty rows above the text.
top_margin = 1

# Empty rows between the text and the status line.
bottom_margin = 1

# Empty rows between paragraphs.
paragraph_spacing = 1

# Maximum height of an image, in rows.
image_max_height = 20

# Draw images. When false, images show as a text placeholder.
images = true

# Auto-scroll speed, in words per minute.
auto_scroll_wpm = 200

# Reading speed, in words per minute, used for the time estimate.
reading_wpm = 250

# Scroll through all chapters as one stream.
continuous = false

# Draw a color bar left of each paragraph, cycling through five theme colors.
paragraph_colors = false

# Mark the current paragraph and mute the others; advance with Space or j/k.
paragraph_mode = false

# Optional KOSync / CrossPoint Sync progress synchronization.
[sync]
enabled = false
server = "https://sync.crosspointreader.com"
username = "you"
password = "secret"
# "binary" matches bytes (KOReader's partial MD5), "filename" hashes the name.
document_id = "binary"
# device = "slurp"
# device_id = "slurp"
# key = "lowercase md5 of the password, instead of password"

# Optional per-role colors. Values are "#rrggbb".
[colors]
text = "#cdd6f4"
background = "#1e1e2e"
heading = "#cba6f7"
accent = "#74c7ec"
muted = "#a6adc8"
search = "#f9e2af"
search_text = "#1e1e2e"
status_bg = "#181825"
status_fg = "#cdd6f4"
border = "#585b70"
selection = "#313244"
```

List the built-in themes:

```sh
slurp --list-themes
```

A theme file can also come from opaline's theme format:

```toml
theme = "/home/me/themes/my-theme.toml"
```

## Images

slurp queries the terminal for a graphics protocol and its cell size:

- Kitty, Ghostty, and supported Sixel terminals use their native graphics
  protocol.
- iTerm2, WezTerm, and compatible terminals use iTerm2 inline images.
- Inside tmux, passthrough is used when tmux and the outer terminal support it.
- Terminals that do not answer, or have no graphics protocol, use Unicode
  halfblocks, which works in any 256-color terminal.

Images are read when they first become visible. Large chapters open quickly
even when they contain many images.

## Reading position

Positions are saved in `$XDG_DATA_HOME/slurp/positions.toml`, usually
`~/.local/share/slurp/positions.toml`. The file keeps the chapter, the scroll
fraction, and the selected paragraph for each book. A position is saved when
the chapter changes, when you quit, and when the program exits.

## By paragraph

Set `paragraph_mode = true` or press `P`. The paragraph at the top of the page
is marked by a bar in the theme's primary accent color and keeps the normal
text color. Other body paragraphs use the theme's muted color; headings,
quotes, links, and code keep their own colors. Space and `j` select the next
paragraph without scrolling; `k` selects the previous one. When the
selection reaches the last paragraph on the page, the next advance puts the
following paragraph at the top of the screen. If the last paragraph is clipped
at the bottom, selecting it scrolls it to the top first. Press `z` to put the
selected paragraph at the top without changing the selection. The selected
paragraph is saved with the reading position, so reopening the book selects it
again.

## Progress sync

The optional `[sync]` table connects slurp to a KOSync-compatible server,
including CrossPoint Sync and `sync.koreader.rocks`. The password is sent as
its lowercase MD5 hash, as KOReader does; use `key` to store that hash instead
of the plain password.

On open, slurp fetches the remote position and uses it when the server's
timestamp is newer than the local one. On exit it pushes the current position.
A failed sync at startup keeps the local position and skips the push, so an
unreachable server cannot overwrite newer progress.

Books are matched by KOReader's partial MD5 with `document_id = "binary"`, or
by file name with `document_id = "filename"`. Use the same mode as the other
device. CrossPoint Sync keeps one position per `device_id`; the default is
`slurp`.

Slurp exchanges whole-book percentages. It does not generate KOReader
XPointers, so a KOReader device resumes at the percentage rather than an exact
element.

Press `r` in the reader to fetch the newest position again and jump to it,
even when its timestamp is not newer than the local one.

## Command line

```
slurp [OPTIONS] [BOOK.epub]

Without a book, slurp opens the recent-books list. `--resume` opens the most
recently read book instead.

  -c, --config <FILE>   Use a specific config file
      --state <FILE>    Use a specific reading position file
  -t, --theme <NAME>    Start with a built-in theme
  -w, --width <COLS>    Set the text column width
  -r, --resume          Open the most recently read book
      --list-themes     List the built-in themes
  -h, --help            Show help
  -V, --version         Show the version
```

## License

MIT
