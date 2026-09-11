---
type: concept
title: TUI app loop
description: Terminal lifecycle, draw loop, scroll state, and key bindings for the viewer.
resource: src/main.rs
tags: [tui, ratatui, crossterm]
---

# TUI app loop

## Lifecycle

`main` (`src/main.rs:58-66`) parses CLI args via `clap` (`Args { file: PathBuf }`, `src/main.rs:22-27`), builds `App` (which eagerly reads and renders the file — `src/main.rs:36-44`), enters the terminal (`setup_terminal`, `src/main.rs:68-73`: raw mode + alt screen), runs the event loop, then unconditionally restores the terminal (`restore_terminal`, `src/main.rs:75-80`: disable raw mode, leave alt screen, show cursor) before propagating `run`'s result. Restoration runs even if `run` returns `Err`, since it's called on the line after `run` rather than inside a `?`-chained expression — this prevents leaving the user's terminal in raw/alt-screen mode on error.

## App state

`App` (`src/main.rs:65-73`) holds: `path` (current file, doubles as the window title and, via its parent dir, the base for resolving relative link targets), `body: Text<'static>` + `links: Vec<Link>` (both from `markdown::render`, not re-rendered per frame), `scroll: u16`, `selected_link: Option<usize>` (index into `links`, for keyboard navigation), `history: Vec<(PathBuf, u16)>` (back-stack of `(path, scroll)` pairs pushed on forward navigation), and `status: Option<String>` (transient message shown in place of the scroll-position status line — set by link-follow outcomes, cleared on the next key press other than Tab/Shift-Tab).

`App::load` (`src/main.rs:90-100`) is the shared path for both initial load (`App::new`, `src/main.rs:76-88`) and navigating to a new file: it re-reads and re-renders, replacing `path`/`body`/`links` and resetting `scroll`/`selected_link` to 0/`None`. It does not touch `history` — callers manage the back-stack around it.

`max_scroll` (`src/main.rs:46-49`) clamps scrolling to `total_lines - viewport_height` (saturating, so it's `0` once content fits the viewport). `scroll_by` (`src/main.rs:51-55`) applies a signed delta and clamps into `[0, max_scroll]`; callers pass `i32::MIN/2` / `i32::MAX/2` for "jump to top/bottom" (`src/main.rs:129-130`) rather than `i32::MIN/MAX` directly, avoiding overflow when the delta is later added to `scroll as i32`.

## Draw loop

`run` (`src/main.rs:82-138`) loops: draw a frame, then poll for input with a 250ms timeout (`src/main.rs:114`) so the loop stays responsive without busy-waiting. Layout is two rows — `Constraint::Min(1)` body + `Constraint::Length(1)` status bar (`src/main.rs:87-88`). `body_height` is recomputed every frame from the actual rendered chunk height minus 2 (border rows) (`src/main.rs:90`) and captured via closure into the outer scope so the event-handling code below can use the same value for scroll clamping — this means scroll math always reflects the last-drawn frame's size, not the current terminal size if a resize event hasn't been drawn yet.

The body is a bordered `Paragraph` wrapping `app.body.clone()` with `Wrap { trim: false }` and `.scroll((app.scroll, 0))` (`src/main.rs:92-99`) — `app.body` is cloned every frame since `Paragraph::new` takes ownership.

## Key bindings

Only `KeyEventKind::Press` is handled (`src/main.rs:116-118`), which matters on Windows/some terminals that also emit `Release`/`Repeat` key events under crossterm's enhanced keyboard protocol — without this filter those would double-trigger scroll actions.

| Key | Action |
|---|---|
| `q`, `Esc` | quit (`src/main.rs:120`) |
| `j`, `Down` | scroll +1 line (`src/main.rs:121`) |
| `k`, `Up` | scroll -1 line (`src/main.rs:122`) |
| `d`, `PageDown` | scroll +half viewport (`src/main.rs:123-125`) |
| `u`, `PageUp` | scroll -half viewport (`src/main.rs:126-128`) |
| `g`, `Home` | jump to top (`src/main.rs:129`) |
| `G`, `End` | jump to bottom (`src/main.rs:130`) |
| `Tab` | select next link, scrolling it into view (`src/main.rs:124-137`, bound `src/main.rs`) |
| `Shift+Tab` | select previous link |
| `Enter` | follow the selected link (`follow_selected`, `src/main.rs:160-165`) |
| `Backspace` | go back to the previous file/scroll position (`go_back`, `src/main.rs:167-175`) |

The status bar (`src/main.rs`, `status_text` in the draw closure) shows `app.status` when set, otherwise the default scroll-position + key-hint line — so a link-follow outcome (external link, not-found target, no-previous-page, etc.) replaces the hint line until the next non-Tab key.

## Link navigation

`resolve_target` (`src/main.rs:46-63`) classifies a link's raw `target` string into a `Target`: `Anchor` for a bare `#fragment` (same-file heading links — not implemented, reported via status only), `External` for anything with a `://` or `mailto:` scheme (not opened — no process is spawned for it), `File` for a relative path that joins to an existing file under `current_file`'s parent directory, else `NotFound`. `App::follow` (`src/main.rs:139-158`) drives the actual state change: only `Target::File` mutates anything (pushes `(old_path, old_scroll)` onto `history` then calls `load`); every other variant just sets `status` to an explanatory message.

Selection and click share one underlying mechanism but diverge at the last step: `Tab`/`Shift+Tab` move `selected_link` and call `ensure_line_visible` (`src/main.rs:113-122`) to scroll the target line into the viewport without auto-following; `Enter` then calls `follow_selected` (`src/main.rs:160-165`), which clones the selected link's target and calls `follow`. A mouse left-click instead calls `link_at` (`src/main.rs:178-185`) to hit-test the click position directly against `links`, and on a hit sets `selected_link` *and* immediately calls `follow_selected` in the same step (click = select + open, no separate confirm).

Selected-link highlighting is applied at draw time, not baked into `app.body`: each frame, if `selected_link` is set, the draw closure clones `app.body` (already cloned every frame regardless — see Draw loop) and patches `Modifier::REVERSED` onto just that link's `span_start..span_end` range on its line (`src/main.rs`, inside the `terminal.draw` closure) before building the `Paragraph`. `app.body` itself is never mutated, so switching or clearing the selection needs no re-render through `markdown::render`.

### Mouse hit-testing caveat

Mouse capture is enabled via `EnableMouseCapture`/`DisableMouseCapture` bracketing the terminal session (`src/main.rs:201`, `205-212`). `link_at` (`src/main.rs:178-185`) and the click handler (inside `run`'s event loop, `MouseEventKind::Down(MouseButton::Left)` arm) map a screen `(row, col)` to a logical line index as `app.scroll as usize + (row - body_area.y - 1)` — i.e. it assumes one logical `Line` renders to exactly one terminal row. This breaks under `Wrap { trim: false }` (`src/main.rs`, draw closure) whenever a logical line is wide enough to soft-wrap into multiple rows: clicks past the first wrapped line of any preceding long line will hit-test against the wrong logical line. `max_scroll`/`scroll_by`'s line-based (pre-wrap) accounting has the same underlying assumption, so this isn't a new inconsistency, just one now surfaced through click coordinates too.

## Known gap

No file-watch/reload — `App::load` reads the file once per navigation (startup or link-follow); external edits to the currently-open markdown file aren't picked up without reopening it.
