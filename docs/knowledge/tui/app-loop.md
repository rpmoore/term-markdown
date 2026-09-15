---
type: concept
title: TUI app loop
description: Terminal lifecycle, draw loop, scroll state, and key bindings for the viewer.
resource: src/main.rs
tags: [tui, ratatui, crossterm]
---

# TUI app loop

## Lifecycle

`main` (`src/main.rs:361-378`) parses CLI args via `clap` (`Args { file: PathBuf, root:
Option<PathBuf>, scheme: Option<String> }`, `src/main.rs:71-82`), fixes the bundle root for
absolute links — `--root` if given (validated and canonicalized by `explicit_root`,
`src/main.rs:382-390`), else `bundle::detect_root(&args.file)` (see
[bundle-root](bundle-root.md)) — resolves the active color scheme via `scheme::Scheme::load`
(see [scheme-loading](../config/scheme-loading.md)), all three of which fail *before* the
terminal is touched, builds `App` (which eagerly reads and renders the file — `App::new`,
`src/main.rs:200-214`), enters the terminal (`setup_terminal`, `src/main.rs:392-397`: raw mode +
alt screen + mouse capture), runs the event loop, then unconditionally restores the terminal
(`restore_terminal`, `src/main.rs:399-408`: disable raw mode, disable mouse capture, leave alt
screen, show cursor) before propagating `run`'s result. Restoration runs even if `run` returns
`Err`, since it's called on the line after `run` rather than inside a `?`-chained expression —
this prevents leaving the user's terminal in raw/alt-screen/mouse-capture mode on error.

## App state

`App` (`src/main.rs:185-197`) holds: `path` (current file, doubles as the window title and, via
its parent dir, the base for resolving *relative* link targets), `bundle_root: PathBuf` (base
for absolute `/x` link targets — fixed in `main` at startup and never touched by
`load`/`go_back`, so climbing out of the bundle via a `../` link doesn't move it), `scheme:
Scheme` (active color scheme — also fixed at startup and never touched by `load`; no in-app
scheme switching or live-reload), `body: Text<'static>` + `links: Vec<Link>` (both from
`markdown::render`, not re-rendered per frame), `scroll: u16` (a **display-row** offset — see
Row accounting below, not a logical-line index), `selected_link: Option<usize>` (index into
`links`, for keyboard navigation), `history: Vec<(PathBuf, u16)>` (back-stack of `(path,
scroll)` pairs pushed on forward navigation), and `status: Option<String>` (transient message
shown in place of the scroll-position status line — set by link-follow outcomes and mouse-click
misses, cleared on the next key press other than Tab/Shift-Tab).

`App::load` (`src/main.rs:216-226`) is the shared path for both initial load (`App::new`) and
navigating to a new file: it re-reads and calls `markdown::render(&source, &self.scheme)`,
replacing `path`/`body`/`links` and resetting `scroll`/`selected_link` to 0/`None`. It does not
touch `history` or `scheme` — callers manage the back-stack around it, and the scheme never
changes after startup.

## Row accounting

`ratatui::widgets::Paragraph`'s `.scroll((y, x))` offset counts **wrapped display rows**, not
logical `Line`s — with wrapping enabled, its render path skips `scroll.y` rows yielded by its
internal word-wrapper before rendering (confirmed against ratatui's `Paragraph` render source).
Since `app.body.lines.len()` counts logical lines, using it directly for scroll clamping or for
mapping a clicked screen row back to a link (as an earlier version of this code did) drifts as
soon as any earlier line word-wraps — every logical line after the first wrapped one lands on
the wrong screen row.

`wrapped_row_count(text, width)` (`src/main.rs:39-66`) reimplements ratatui's greedy word-wrap
closely enough to count rows correctly (ratatui's own wrapper, `WordWrapper`, lives in a private
module and isn't reusable). Its row count saturates at `u16::MAX` rather than truncating via `as
u16` — `Paragraph::scroll` itself takes a `u16` offset, so a document can never scroll past that
many rows anyway, and a wraparound-truncated count (e.g. a pathologically long unbroken line
landing on a small or zero value) would corrupt every line's accumulated offset after it, not
just cap that one line. `App::row_starts(width)` (`src/main.rs:234-244`) builds the cumulative
per-line row offsets from it via `saturating_add` — length `lines.len() + 1`, with the trailing
entry equal to the total row count (itself capped at `u16::MAX` once enough lines accumulate).
`max_scroll`, `scroll_by`, and `ensure_line_visible` (`src/main.rs:261-284`) all clamp/compute
against this total rather than `lines.len()` — `ensure_line_visible` uses saturating arithmetic
throughout (`row.saturating_add(1).saturating_sub(viewport_height)`, guarded by
`row >= self.scroll.saturating_add(viewport_height)`) since `row` can legitimately be `u16::MAX`
for a large enough document, and plain `row + 1` there would overflow. `line_at_row(row, width)`
(`src/main.rs:249-259`) is the inverse: binary-searches `row_starts` (`partition_point`) to turn
a screen row back into `(logical_line, sub_row_within_line)`.

This recomputes `row_starts` (an O(lines) pass with a per-line `String` allocation) on every
draw and on every mouse click — acceptable for documents in the tens-to-hundreds of lines this
viewer targets, not cached beyond that.

## Draw loop

`run` (`src/main.rs:410-541`) loops: draw a frame, then poll for input with a 250ms timeout so
the loop stays responsive without busy-waiting. Layout is two rows — `Constraint::Min(1)` body +
`Constraint::Length(1)` status bar. `body_height`, `body_area`, and `content_width`
(border-adjusted body width, i.e. `chunks[0].width - 2`, matching the width ratatui itself wraps
at) are recomputed every frame and captured via closure into the outer scope so the
event-handling code below can use the same values — this means scroll/click math always reflects
the last-drawn frame's size, not the current terminal size if a resize event hasn't been drawn
yet. `app.scroll` is also reclamped against the current frame's `max_scroll` at the top of the
draw closure, so a resize that shrinks the effective row count (or rewraps content narrower)
can't leave `scroll` pointing past the end.

The body is a bordered `Paragraph` wrapping `app.body.clone()` with `Wrap { trim: false }` and
`.scroll((app.scroll, 0))` — `app.body` is cloned every frame since `Paragraph::new` takes
ownership. Chrome styling comes from `app.scheme.ui` (see
[scheme-loading](../config/scheme-loading.md)): if `ui.background` is `Some`, the draw closure
fills the whole frame area with it before rendering anything else (new behavior — the built-in
default scheme's `background` is `None`, so this is a no-op for zero-config users);
`ui.border`/`ui.title` are applied to the content `Block` via `.border_style`/`.title_style`
when `Some`, otherwise the `Block` keeps ratatui's own default styling (unchanged from before
this field existed); the status bar `Paragraph` is styled with `ui.status_bar` unconditionally
(always `Some` in practice — there's no `"none"` variant for it, unlike
background/border/title).

## Key bindings

Only `KeyEventKind::Press` is handled, which matters on Windows/some terminals that also emit
`Release`/`Repeat` key events under crossterm's enhanced keyboard protocol — without this filter
those would double-trigger scroll actions.

| Key | Action |
|---|---|
| `q`, `Esc` | quit |
| `j`, `Down` | scroll +1 row |
| `k`, `Up` | scroll -1 row |
| `d`, `PageDown` | scroll +half viewport |
| `u`, `PageUp` | scroll -half viewport |
| `g`, `Home` | jump to top |
| `G`, `End` | jump to bottom |
| `Tab` | select next link, scrolling it into view (`select_next_link`, `src/main.rs:286-299`) |
| `Shift+Tab` | select previous link |
| `Enter` | follow the selected link (`follow_selected`, `src/main.rs:333-338`) |
| `Backspace` | go back to the previous file/scroll position (`go_back`, `src/main.rs:340-348`) |

The status bar shows `app.status` when set, otherwise the default scroll-position + key-hint
line — so a link-follow outcome (external link, not-found target, no-previous-page,
click-related messages, etc.) replaces the hint line until the next non-Tab key.

## Link navigation

`resolve_target` (`src/main.rs:108-139`) classifies a link's raw `target` string into a
`Target`, given a `LinkBase { current_file, bundle_root }` (`src/main.rs:101-106` — a named pair
rather than two positional `&Path`s, so the two bases can't be silently swapped). `Anchor` for a
bare `#fragment` (same-file heading links — not implemented, reported via status only);
`External` for anything with a `://` or `mailto:` scheme (not opened — no process is spawned for
it) and for a `//host` protocol-relative URL (`src/main.rs:120-122`), which must never be
mistaken for a bundle-absolute path; otherwise the fragment is dropped and the path is probed
for an existing file:

- **absolute** (`/x/y.md`, `src/main.rs:123-131`): per OKF §5.1 these are *bundle-relative*, so
  the candidates are `<bundle_root>/x/y.md` first, then the literal filesystem path `/x/y.md`
  (for tool-generated links that really do point at the host filesystem). The bundle candidate
  has its `.`/`..` segments resolved and clamped at the root first (`clamp_to_root`,
  `src/main.rs:144-156`), so `/../x.md` can't escape to a sibling of the bundle. The bundle root
  wins when both exist.
- **relative** (anything else, `src/main.rs:131-138`): joined to `current_file`'s parent
  directory, with no clamping to the bundle — links that deliberately climb out of the bundle
  (`../../src/x.md`) work.

`probe` (`src/main.rs:162-175`) takes the first candidate that `is_file()`; if none does and the
path ends in a `:<digits>` location suffix (`strip_line_suffix`, `src/main.rs:179-183`), it
retries with that stripped, up to twice so `foo.go:154:12` also resolves. The line number itself
is discarded (no scroll-to-line). Otherwise `NotFound` carries the first candidate for the path
*as written*, and `follow` appends `(bundle root: …)` to the status message for absolute links
so a misdetected root — fixable with `--root` — is obvious.

`App::follow` (`src/main.rs:301-331`) drives the actual state change: only `Target::File`
mutates anything (pushes `(old_path, old_scroll)` onto `history` then calls `load`); every other
variant just sets `status` to an explanatory message.

Selection and click share one underlying mechanism but diverge at the last step:
`Tab`/`Shift+Tab` move `selected_link` and call `ensure_line_visible` to scroll the target line
into the viewport without auto-following; `Enter` then calls `follow_selected`, which clones the
selected link's target and calls `follow`. A mouse left-click instead resolves the clicked
screen row via `line_at_row`, and — only when the click landed on row 0 of its logical line (see
Mouse hit-testing below) — calls `link_at` (`src/main.rs:351-358`) to hit-test the column
against `links`; on a hit it sets `selected_link` *and* immediately calls `follow_selected` in
the same step (click = select + open, no separate confirm).

Selected-link highlighting is applied at draw time, not baked into `app.body`: each frame, if
`selected_link` is set, the draw closure clones `app.body` (already cloned every frame
regardless — see Draw loop) and patches `app.scheme.ui.selection` onto just that link's
`span_start..span_end` range on its line before building the `Paragraph` (the built-in default
scheme sets `selection` to `Modifier::REVERSED`, matching the original hardcoded behavior).
`app.body` itself is never mutated, so switching or clearing the selection needs no re-render
through `markdown::render`.

## Mouse hit-testing

Mouse capture is enabled via `EnableMouseCapture`/`DisableMouseCapture` bracketing the terminal
session. The click handler (inside `run`'s event loop, `MouseEventKind::Down(MouseButton::Left)`
arm) converts the click's screen `(row, col)` into a body-relative `(row, col)` via
`body_area`'s origin, then calls `app.line_at_row(row, content_width)` to get `(logical_line,
sub_row)`:

- `sub_row == 0` (click landed on a line's first display row, i.e. its un-wrapped start): `col`
  lines up exactly with `link_col_range`'s column space (both measured from the logical line's
  start), so `link_at(line, col)` is called directly.
- `sub_row > 0` (click landed on the *wrapped continuation* of a long line): the column space of
  that screen row doesn't correspond to any column offset we've computed (we only track row
  counts, not each wrap's character break point), so no hit-test is attempted — the status bar
  reports which line/sub-row was clicked and suggests `Tab` instead, rather than guessing with a
  wrong column.

In practice this residual gap only matters for a link embedded partway into a line long enough
to wrap itself; a link on its own short line (the common case, e.g. list-item links) is always
`sub_row == 0` regardless of how much *earlier* content has wrapped, since
`row_starts`/`line_at_row` account for that correctly.

## Known gap

No file-watch/reload — `App::load` reads the file once per navigation (startup or link-follow);
external edits to the currently-open markdown file aren't picked up without reopening it.
