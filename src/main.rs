mod bundle;
mod markdown;
mod scheme;

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clap::Parser as ClapParser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use markdown::Link;
use scheme::Scheme;

/// How many terminal rows `text` occupies once word-wrapped at `width`
/// columns, matching ratatui's `Wrap { trim: false }` behavior closely
/// enough to map screen rows back to logical lines (ratatui's own wrapper
/// lives in a private module, so this is a reimplementation, not a call-out
/// to it). A width of 0, or empty text, always occupies exactly one row.
/// Returns `u32`, not `u16`: `ratatui::Paragraph::scroll` itself takes a
/// `u16` offset, but row *counts* and *cumulative totals* (see
/// `App::row_starts`) need headroom above that to avoid losing precision
/// right at the boundary — clamping every individual count to `u16::MAX`
/// before summing would make a document with, say, exactly 65,536 total
/// rows indistinguishable from one with 65,535, undercounting the true
/// total by one and leaving its last row permanently one row outside any
/// viewport. Only the final scroll offset actually handed to ratatui needs
/// clamping to `u16`, not the counts feeding into it.
fn wrapped_row_count(text: &str, width: u16) -> u32 {
    if width == 0 || text.is_empty() {
        return 1;
    }
    let width = width as usize;
    let mut rows: usize = 1;
    let mut col: usize = 0;
    for chunk in text.split_inclusive(' ') {
        let w = chunk.width();
        if w > width {
            if col > 0 {
                rows += 1;
            }
            let mut remaining = w;
            while remaining > width {
                rows += 1;
                remaining -= width;
            }
            col = remaining;
        } else if col + w > width {
            rows += 1;
            col = w;
        } else {
            col += w;
        }
    }
    rows.min(u32::MAX as usize) as u32
}

/// Terminal markdown viewer.
#[derive(ClapParser)]
#[command(name = "term-markdown", version, about)]
struct Args {
    /// Markdown file to view
    file: PathBuf,

    /// Bundle root for absolute (`/x/y.md`) links; auto-detected when omitted
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Color scheme to use, overriding ~/.term-markdown/config.toml
    #[arg(long, value_name = "NAME")]
    scheme: Option<String>,
}

/// Where a link points: relative paths resolve against the file it appeared
/// in, absolute (`/x/y.md`) paths against the OKF bundle root.
enum Target {
    /// A markdown file on disk that exists and can be navigated to.
    File(PathBuf),
    /// Has a URL scheme (http, mailto, ...) — not opened, just reported.
    External(String),
    /// A same-file `#fragment` anchor — no heading-scroll support yet.
    Anchor(String),
    /// Resolved to a local path that doesn't exist.
    NotFound(PathBuf),
}

/// The two paths a link is resolved against. Both are borrowed from `App`;
/// bundling them keeps `resolve_target` from taking two positional `&Path`s
/// that would silently accept being swapped.
#[derive(Clone, Copy)]
struct LinkBase<'a> {
    /// File the link appeared in — base for relative targets.
    current_file: &'a Path,
    /// OKF bundle root — base for `/`-prefixed targets.
    bundle_root: &'a Path,
}

fn resolve_target(base: LinkBase<'_>, target: &str) -> Target {
    if let Some((path_part, fragment)) = target.split_once('#')
        && path_part.is_empty()
    {
        return Target::Anchor(fragment.to_string());
    }
    let path_part = target.split('#').next().unwrap_or(target);
    if path_part.contains("://") || path_part.starts_with("mailto:") {
        return Target::External(target.to_string());
    }
    // A protocol-relative URL ("//host/path") is not a local path at all —
    // treat it like any other external link rather than a bundle-root lookup.
    if path_part.starts_with("//") {
        return Target::External(target.to_string());
    }
    if path_part.starts_with('/') {
        // OKF §5.1: absolute links are bundle-relative. Fall back to the
        // literal filesystem path for tool-generated links that really do
        // point at the host filesystem.
        let root = base.bundle_root.to_path_buf();
        probe(path_part, |p| {
            vec![root.join(clamp_to_root(p)), PathBuf::from(p)]
        })
    } else {
        let dir = base
            .current_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        probe(path_part, |p| vec![dir.join(p)])
    }
}

/// `path` as a relative path with `.`/`..` segments resolved and clamped at
/// the top, so a bundle-absolute `/../x.md` or `/a/../../x.md` still lands
/// inside the bundle root rather than a sibling of it.
fn clamp_to_root(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out
}

/// Return the first existing file among `candidates(path)`; failing that,
/// retry with a trailing `:LINE` (and then `:LINE:COL`) stripped, since
/// tool-generated links often carry a location suffix. `NotFound` reports
/// the first candidate for the path as written.
fn probe(path: &str, candidates: impl Fn(&str) -> Vec<PathBuf>) -> Target {
    let first = candidates(path);
    let mut attempt = path;
    for _ in 0..3 {
        if let Some(hit) = candidates(attempt).into_iter().find(|p| p.is_file()) {
            return Target::File(hit);
        }
        match strip_line_suffix(attempt) {
            Some(stripped) => attempt = stripped,
            None => break,
        }
    }
    Target::NotFound(first.into_iter().next().unwrap_or_default())
}

/// `path` without a trailing `:<digits>` location suffix, or `None` when
/// there is no such suffix (so `foo:bar.md` and `a.go:` are left alone).
fn strip_line_suffix(path: &str) -> Option<&str> {
    let (head, tail) = path.rsplit_once(':')?;
    (!head.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))
        .then_some(head)
}

struct App {
    path: PathBuf,
    /// Base for absolute links; fixed at startup, never changed by `load`.
    bundle_root: PathBuf,
    /// Active color scheme; fixed at startup, never changed by `load`.
    scheme: Scheme,
    body: Text<'static>,
    links: Vec<Link>,
    scroll: u16,
    selected_link: Option<usize>,
    history: Vec<(PathBuf, u16)>,
    status: Option<String>,
}

impl App {
    fn new(path: PathBuf, bundle_root: PathBuf, scheme: Scheme) -> Result<Self> {
        let mut app = App {
            path: PathBuf::new(),
            bundle_root,
            scheme,
            body: Text::default(),
            links: Vec::new(),
            scroll: 0,
            selected_link: None,
            history: Vec::new(),
            status: None,
        };
        app.load(path)?;
        Ok(app)
    }

    fn load(&mut self, path: PathBuf) -> Result<()> {
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let rendered = markdown::render(&source, &self.scheme);
        self.path = path;
        self.body = rendered.text;
        self.links = rendered.links;
        self.scroll = 0;
        self.selected_link = None;
        Ok(())
    }

    /// Cumulative display-row start of each logical line at `width` columns,
    /// plus a trailing sentinel equal to the total row count. `ratatui`'s
    /// `Paragraph::scroll` offset counts wrapped display rows, not logical
    /// lines (see `render_text` in ratatui's paragraph widget), so anything
    /// mapping a screen row back to a logical line — or clamping scroll —
    /// needs this rather than `self.body.lines.len()`. Kept as `u32`, wider
    /// than the `u16` `Paragraph::scroll` itself takes: only the scroll
    /// offset ultimately handed to ratatui needs clamping to `u16` (see
    /// `ensure_line_visible`), not the totals feeding into it, so a document
    /// landing exactly on the `u16` boundary doesn't lose a row to premature
    /// rounding before it even gets there.
    fn row_starts(&self, width: u16) -> Vec<u32> {
        let mut starts = Vec::with_capacity(self.body.lines.len() + 1);
        let mut acc: u32 = 0;
        for line in &self.body.lines {
            starts.push(acc);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            acc = acc.saturating_add(wrapped_row_count(&text, width));
        }
        starts.push(acc);
        starts
    }

    /// The logical line and within-line display row that screen `row`
    /// (0-based, counted from the top of scrolled content) falls on, or
    /// `None` if `row` is past the end of the document.
    fn line_at_row(&self, row: u32, width: u16) -> Option<(usize, u32)> {
        let starts = self.row_starts(width);
        let total = *starts.last().unwrap();
        if row >= total {
            return None;
        }
        let n = starts.len() - 1;
        let idx = starts[..n].partition_point(|&s| s <= row);
        let line = idx.saturating_sub(1);
        Some((line, row - starts[line]))
    }

    fn max_scroll(&self, viewport_height: u16, width: u16) -> u32 {
        let total = *self.row_starts(width).last().unwrap();
        total.saturating_sub(viewport_height as u32)
    }

    /// `max_scroll` clamped to what `self.scroll` (a `u16`, matching
    /// `ratatui::Paragraph::scroll`'s own offset type) can actually hold. A
    /// document whose total row count genuinely exceeds what a `u16` scroll
    /// offset can reach is an unavoidable ratatui limitation — its tail is
    /// simply not reachable — but that's a distinct, larger-scale problem
    /// from losing a row right at the boundary through internal rounding,
    /// which is what keeping `row_starts`/`max_scroll` in `u32` avoids.
    fn max_scroll_u16(&self, viewport_height: u16, width: u16) -> u16 {
        self.max_scroll(viewport_height, width).min(u16::MAX as u32) as u16
    }

    fn scroll_by(&mut self, delta: i32, viewport_height: u16, width: u16) {
        let max = self.max_scroll_u16(viewport_height, width);
        let new = (self.scroll as i32 + delta).clamp(0, max as i32);
        self.scroll = new as u16;
    }

    fn ensure_line_visible(&mut self, line: usize, viewport_height: u16, width: u16) {
        let row = self.row_starts(width)[line];
        if row < self.scroll as u32 {
            self.scroll = row as u16; // row < self.scroll <= u16::MAX, so this fits.
        } else if viewport_height > 0 && row >= self.scroll as u32 + viewport_height as u32 {
            // Computed in `u32`, not `row + 1 - viewport_height` in `u16`:
            // `row` can be `u16::MAX` for a document with enough wrapped
            // rows, and `row + 1` there would either overflow (unchecked)
            // or saturate back down to `u16::MAX` (saturating) — either way
            // undercounting by exactly one and leaving `line` one row
            // outside the viewport even though it should just barely fit.
            let target = (row + 1).saturating_sub(viewport_height as u32);
            self.scroll = target.min(u16::MAX as u32) as u16;
        }
        let max = self.max_scroll_u16(viewport_height, width);
        self.scroll = self.scroll.min(max);
    }

    fn select_next_link(&mut self, forward: bool, viewport_height: u16, width: u16) {
        if self.links.is_empty() {
            self.status = Some("no links in this document".to_string());
            return;
        }
        let next = match self.selected_link {
            None => 0,
            Some(i) if forward => (i + 1) % self.links.len(),
            Some(i) => (i + self.links.len() - 1) % self.links.len(),
        };
        self.selected_link = Some(next);
        let line = self.links[next].line;
        self.ensure_line_visible(line, viewport_height, width);
    }

    fn follow(&mut self, target: &str) {
        let base = LinkBase {
            current_file: &self.path,
            bundle_root: &self.bundle_root,
        };
        match resolve_target(base, target) {
            Target::File(path) => {
                let from = (self.path.clone(), self.scroll);
                match self.load(path) {
                    Ok(()) => self.history.push(from),
                    Err(e) => self.status = Some(format!("failed to open link: {e}")),
                }
            }
            Target::External(url) => {
                self.status = Some(format!("external link (not opened): {url}"));
            }
            Target::Anchor(fragment) => {
                self.status = Some(format!("in-page anchors not supported yet: #{fragment}"));
            }
            Target::NotFound(path) => {
                // Name the bundle root for absolute links so a misdetected
                // root (fixable with --root) is obvious from the message.
                let hint = if target.starts_with('/') {
                    format!(" (bundle root: {})", self.bundle_root.display())
                } else {
                    String::new()
                };
                self.status = Some(format!("link target not found: {}{hint}", path.display()));
            }
        }
    }

    fn follow_selected(&mut self) {
        if let Some(idx) = self.selected_link {
            let target = self.links[idx].target.clone();
            self.follow(&target);
        }
    }

    fn go_back(&mut self) {
        if let Some((path, scroll)) = self.history.pop() {
            if self.load(path).is_ok() {
                self.scroll = scroll;
            }
        } else {
            self.status = Some("no previous page".to_string());
        }
    }

    /// Link (if any) whose rendered column range on `line` contains `col`.
    fn link_at(&self, line: usize, col: u16) -> Option<usize> {
        self.links.iter().position(|link| {
            link.line == line && {
                let (start, end) = markdown::link_col_range(&self.body.lines[link.line], link);
                col >= start && col < end
            }
        })
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bundle_root = match args.root {
        Some(root) => explicit_root(root)?,
        None => bundle::detect_root(&args.file).context("failed to detect bundle root")?,
    };
    let scheme = Scheme::load(
        scheme::default_config_path().as_deref(),
        args.scheme.as_deref(),
    )
    .context("failed to load color scheme")?;
    let mut app = App::new(args.file, bundle_root, scheme)?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

/// Validate a user-supplied `--root` and canonicalize it so status messages
/// don't carry `..` components around.
fn explicit_root(root: PathBuf) -> Result<PathBuf> {
    ensure!(
        root.is_dir(),
        "--root {} is not a directory",
        root.display()
    );
    std::fs::canonicalize(&root)
        .with_context(|| format!("failed to resolve --root {}", root.display()))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut body_height: u16 = 0;
    let mut body_area = Rect::default();
    let mut content_width: u16 = 0;

    loop {
        terminal.draw(|frame| {
            let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());

            body_area = chunks[0];
            body_height = chunks[0].height.saturating_sub(2);
            content_width = chunks[0].width.saturating_sub(2);

            app.scroll = app.scroll.min(app.max_scroll_u16(body_height, content_width));

            if let Some(bg) = app.scheme.ui.background {
                frame.render_widget(Block::default().style(bg), frame.area());
            }

            let mut text = app.body.clone();
            if let Some(idx) = app.selected_link {
                let link = &app.links[idx];
                if let Some(line) = text.lines.get_mut(link.line) {
                    for span in &mut line.spans[link.span_start..link.span_end] {
                        span.style = span.style.patch(app.scheme.ui.selection);
                    }
                }
            }

            let mut block = Block::default()
                .borders(Borders::ALL)
                .title(app.path.display().to_string());
            if let Some(border_style) = app.scheme.ui.border {
                block = block.border_style(border_style);
            }
            if let Some(title_style) = app.scheme.ui.title {
                block = block.title_style(title_style);
            }
            let paragraph = Paragraph::new(text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll, 0));
            frame.render_widget(paragraph, chunks[0]);

            let max = app.max_scroll(body_height, content_width);
            let hint = "q: quit  j/k: scroll  g/G: top/bottom  Tab: next link  Enter: open  Backspace: back";
            let status_text = match &app.status {
                Some(msg) => format!(" {msg}"),
                None => format!(" line {}/{}  |  {}", app.scroll, max, hint),
            };
            frame.render_widget(
                Paragraph::new(Line::from(status_text)).style(app.scheme.ui.status_bar),
                chunks[1],
            );
        })?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                        app.status = None;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('j') | KeyCode::Down => {
                            app.scroll_by(1, body_height, content_width)
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.scroll_by(-1, body_height, content_width)
                        }
                        KeyCode::Char('d') | KeyCode::PageDown => {
                            app.scroll_by(body_height as i32 / 2, body_height, content_width)
                        }
                        KeyCode::Char('u') | KeyCode::PageUp => {
                            app.scroll_by(-(body_height as i32) / 2, body_height, content_width)
                        }
                        KeyCode::Char('g') | KeyCode::Home => {
                            app.scroll_by(i32::MIN / 2, body_height, content_width)
                        }
                        KeyCode::Char('G') | KeyCode::End => {
                            app.scroll_by(i32::MAX / 2, body_height, content_width)
                        }
                        KeyCode::Tab => app.select_next_link(true, body_height, content_width),
                        KeyCode::BackTab => app.select_next_link(false, body_height, content_width),
                        KeyCode::Enter => app.follow_selected(),
                        KeyCode::Backspace => app.go_back(),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                        let inside = mouse.row > body_area.y
                            && mouse.row + 1 < body_area.y + body_area.height
                            && mouse.column > body_area.x
                            && mouse.column + 1 < body_area.x + body_area.width;
                        if inside {
                            let row = app.scroll + (mouse.row - body_area.y - 1);
                            let col = mouse.column - body_area.x - 1;
                            match app.line_at_row(row as u32, content_width) {
                                Some((line, 0)) => match app.link_at(line, col) {
                                    Some(idx) => {
                                        app.status = None;
                                        app.selected_link = Some(idx);
                                        app.follow_selected();
                                    }
                                    None => {
                                        app.status = Some(format!(
                                            "click at line {line} col {col}: no link there"
                                        ));
                                    }
                                },
                                Some((line, sub_row)) => {
                                    app.status = Some(format!(
                                        "click on wrapped row {sub_row} of line {line}: not supported yet, use Tab instead"
                                    ));
                                }
                                None => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_kind(target: Target) -> &'static str {
        match target {
            Target::File(_) => "file",
            Target::External(_) => "external",
            Target::Anchor(_) => "anchor",
            Target::NotFound(_) => "not_found",
        }
    }

    fn temp_tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("term-markdown-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "hello").unwrap();
    }

    fn base<'a>(current_file: &'a Path, bundle_root: &'a Path) -> LinkBase<'a> {
        LinkBase {
            current_file,
            bundle_root,
        }
    }

    fn expect_file(target: Target) -> PathBuf {
        match target {
            Target::File(path) => path,
            other => panic!("expected File target, got {}", target_kind(other)),
        }
    }

    fn expect_not_found(target: Target) -> PathBuf {
        match target {
            Target::NotFound(path) => path,
            other => panic!("expected NotFound target, got {}", target_kind(other)),
        }
    }

    const UNUSED_ROOT: &str = "/tmp/term-markdown-unused-root";

    #[test]
    fn resolves_relative_path_to_sibling_file() {
        let dir = temp_tree("test");
        let current = dir.join("current.md");
        let sibling = dir.join("other.md");
        touch(&sibling);

        let resolved = resolve_target(base(&current, Path::new(UNUSED_ROOT)), "./other.md");
        assert_eq!(expect_file(resolved), sibling);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absolute_link_from_nested_file_resolves_against_bundle_root() {
        let t = temp_tree("abs-nested");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        let wanted = root.join("x/y.md");
        touch(&wanted);
        // Decoy: what the old "relative to the file's dir" rule would pick.
        touch(&root.join("sub/x/y.md"));

        let resolved = resolve_target(base(&current, &root), "/x/y.md");
        assert_eq!(expect_file(resolved), wanted);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn absolute_link_parent_segments_are_clamped_at_bundle_root() {
        let t = temp_tree("abs-dotdot");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        let inside = root.join("guide.md");
        touch(&inside);
        // Decoy: what an unclamped `<root>/../guide.md` would open.
        touch(&t.join("guide.md"));

        let b = base(&current, &root);
        assert_eq!(expect_file(resolve_target(b, "/../guide.md")), inside);
        assert_eq!(
            expect_file(resolve_target(b, "/sub/../../guide.md")),
            inside
        );
        assert_eq!(expect_file(resolve_target(b, "/./guide.md")), inside);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn clamp_to_root_cases() {
        assert_eq!(clamp_to_root("/x/y.md"), PathBuf::from("x/y.md"));
        assert_eq!(clamp_to_root("/../x.md"), PathBuf::from("x.md"));
        assert_eq!(clamp_to_root("/a/../b/./c.md"), PathBuf::from("b/c.md"));
        assert_eq!(clamp_to_root("/a//b.md"), PathBuf::from("a/b.md"));
    }

    #[test]
    fn absolute_link_from_root_level_file_resolves_against_bundle_root() {
        let t = temp_tree("abs-rootlevel");
        let current = t.join("index.md");
        let other = t.join("other.md");
        touch(&other);

        let resolved = resolve_target(base(&current, &t), "/other.md");
        assert_eq!(expect_file(resolved), other);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn absolute_link_falls_back_to_literal_filesystem_path() {
        let t = temp_tree("abs-literal");
        let root = t.join("root");
        let current = root.join("current.md");
        let literal = t.join("elsewhere/real.go");
        touch(&literal);

        let target = literal.to_str().unwrap();
        let resolved = resolve_target(base(&current, &root), target);
        assert_eq!(expect_file(resolved), literal);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn bundle_root_wins_over_literal_filesystem_path() {
        let t = temp_tree("abs-precedence");
        let root = t.join("root");
        let current = root.join("current.md");
        let literal = t.join("elsewhere/real.md");
        touch(&literal);
        let mirror = root.join(literal.strip_prefix("/").unwrap());
        touch(&mirror);

        let target = literal.to_str().unwrap();
        let resolved = resolve_target(base(&current, &root), target);
        assert_eq!(expect_file(resolved), mirror);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn line_suffix_is_stripped_for_root_literal_and_relative() {
        let t = temp_tree("abs-linesuffix");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        let in_root = root.join("x/y.md");
        touch(&in_root);
        let literal = t.join("elsewhere/real.go");
        touch(&literal);
        let sibling = root.join("sub/y.go");
        touch(&sibling);
        let lit = literal.to_str().unwrap();

        let b = base(&current, &root);
        assert_eq!(expect_file(resolve_target(b, "/x/y.md:12")), in_root);
        assert_eq!(
            expect_file(resolve_target(b, &format!("{lit}:154"))),
            literal
        );
        assert_eq!(
            expect_file(resolve_target(b, &format!("{lit}:154:7"))),
            literal
        );
        assert_eq!(expect_file(resolve_target(b, "./y.go:3")), sibling);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn absolute_link_fragment_is_dropped_before_resolving() {
        let t = temp_tree("abs-fragment");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        let wanted = root.join("x/y.md");
        touch(&wanted);

        let resolved = resolve_target(base(&current, &root), "/x/y.md#sec");
        assert_eq!(expect_file(resolved), wanted);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn missing_absolute_link_is_not_found_at_root_joined_path() {
        let t = temp_tree("abs-missing");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        std::fs::create_dir_all(&root).unwrap();

        let b = base(&current, &root);
        assert_eq!(
            expect_not_found(resolve_target(b, "/nope.md")),
            root.join("nope.md")
        );
        // Reported as written — the :LINE suffix is not stripped from the message.
        assert_eq!(
            expect_not_found(resolve_target(b, "/nope.go:3")),
            root.join("nope.go:3")
        );

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn relative_link_climbs_out_of_bundle_without_clamping() {
        let t = temp_tree("rel-climb");
        let root = t.join("root");
        let current = root.join("sub/current.md");
        let outside = t.join("outside.md");
        touch(&outside);
        // `..` traversal only works through directories that exist.
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();

        let resolved = resolve_target(base(&current, &root), "../../outside.md");
        assert_eq!(expect_file(resolved), root.join("sub/../../outside.md"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn strip_line_suffix_cases() {
        assert_eq!(strip_line_suffix("a.go:154"), Some("a.go"));
        assert_eq!(strip_line_suffix("a.go:154:7"), Some("a.go:154"));
        assert_eq!(strip_line_suffix("a.md"), None);
        assert_eq!(strip_line_suffix("a:b"), None);
        assert_eq!(strip_line_suffix("a.go:"), None);
        assert_eq!(strip_line_suffix(":12"), None);
        assert_eq!(strip_line_suffix("foo:bar.md"), None);
    }

    #[test]
    fn explicit_root_rejects_non_directory() {
        let t = temp_tree("root-nondir");
        let file = t.join("file.md");
        touch(&file);

        assert!(explicit_root(file).is_err());
        assert!(explicit_root(t.join("missing")).is_err());

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn explicit_root_canonicalizes_path() {
        let t = temp_tree("root-canon");
        let nested = t.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = explicit_root(nested.join("..").join("b")).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&nested).unwrap());

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn protocol_relative_path_is_external() {
        let current = PathBuf::from("/tmp/current.md");
        // A leading "//" is a protocol-relative URL, not a bundle-absolute
        // path — it must not be looked up under the bundle root (which could
        // otherwise silently hit an unrelated local file with a matching name).
        let resolved = resolve_target(
            base(&current, Path::new(UNUSED_ROOT)),
            "//example.com/readme.md",
        );
        assert_eq!(target_kind(resolved), "external");
    }

    #[test]
    fn missing_relative_path_is_not_found() {
        let current = PathBuf::from("/tmp/term-markdown-nonexistent-dir/current.md");
        let resolved = resolve_target(base(&current, Path::new(UNUSED_ROOT)), "./missing.md");
        assert_eq!(target_kind(resolved), "not_found");
    }

    #[test]
    fn http_url_is_external() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(
            base(&current, Path::new(UNUSED_ROOT)),
            "https://example.com",
        );
        assert_eq!(target_kind(resolved), "external");
    }

    #[test]
    fn mailto_is_external() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(
            base(&current, Path::new(UNUSED_ROOT)),
            "mailto:someone@example.com",
        );
        assert_eq!(target_kind(resolved), "external");
    }

    #[test]
    fn bare_fragment_is_anchor() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(base(&current, Path::new(UNUSED_ROOT)), "#some-heading");
        match resolved {
            Target::Anchor(frag) => assert_eq!(frag, "some-heading"),
            _ => panic!("expected Anchor target"),
        }
    }

    #[test]
    fn short_text_takes_one_row() {
        assert_eq!(wrapped_row_count("hello world", 80), 1);
    }

    #[test]
    fn long_text_wraps_across_multiple_rows() {
        let text = "a ".repeat(50); // 100 cols wide
        assert_eq!(wrapped_row_count(&text, 20), 5);
    }

    #[test]
    fn single_overlong_word_force_wraps() {
        let text = "x".repeat(45);
        assert_eq!(wrapped_row_count(&text, 20), 3); // 20 + 20 + 5
    }

    #[test]
    fn row_count_does_not_truncate_past_old_u16_limit() {
        // At width 1, a 70,000-char unbroken line needs exactly 70,000
        // rows — past the old u16::MAX ceiling. Regression test: `rows as
        // u16` used to truncate this to a small (here: near-zero) value
        // instead of preserving it, corrupting every line's row accounting
        // after it. `wrapped_row_count` now returns `u32`, wide enough that
        // no truncation happens at all for a count like this.
        let text = "x".repeat(70_000);
        assert_eq!(wrapped_row_count(&text, 1), 70_000);
    }

    #[test]
    fn selecting_link_at_u16_max_row_is_visible_and_does_not_panic() {
        // Regression test for overflow in `ensure_line_visible`: a link on
        // a logical line whose row offset is exactly `u16::MAX` used to
        // panic (debug) or silently corrupt scroll state (release) via
        // `row + 1 - viewport_height`. It's since been widened to `u32`
        // internally, which also fixes a subtler bug a Codex PR review
        // caught in the first (panic-only) fix: clamping the row count to
        // `u16::MAX` *before* summing made a document with exactly 65,536
        // total rows indistinguishable from one with 65,535, so the last
        // row's link ended up one row outside the viewport even after the
        // scroll math stopped panicking. This asserts the link is actually
        // visible, not just that selecting it doesn't crash.
        let dir = temp_tree("rowlimit");
        let file = dir.join("doc.md");
        // 65,535 hard-broken one-char lines (two trailing spaces force a
        // hard break) place the link's own line at row offset u16::MAX.
        let mut source = "x  \n".repeat(65_535);
        source.push_str("[end](target.md)\n");
        std::fs::write(&file, &source).unwrap();
        touch(&dir.join("target.md"));

        let mut app = App::new(file, dir.clone(), Scheme::default_builtin()).unwrap();
        app.select_next_link(true, 20, 80);

        let link = &app.links[app.selected_link.unwrap()];
        let row = app.row_starts(80)[link.line];
        assert!(
            row >= app.scroll as u32 && row < app.scroll as u32 + 20,
            "link row {row} not visible in viewport [{}, {})",
            app.scroll,
            app.scroll as u32 + 20
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn row_starts_accounts_for_wrapped_lines() {
        let dir =
            std::env::temp_dir().join(format!("term-markdown-rowtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.md");
        // line 0: wraps to 2 rows at width 10 ("0123456789" + "abcde")
        // line 1: single short row
        std::fs::write(&file, "0123456789 abcde\n\nshort\n").unwrap();

        let app = App::new(file, dir.clone(), Scheme::default_builtin()).unwrap();
        let starts = app.row_starts(10);
        // first logical line should take >1 row at this width
        assert!(starts[1] - starts[0] > 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_doc_link_hit_tests_correctly_despite_frontmatter_and_wrapping() {
        // Regression test: docs/knowledge/index.md has frontmatter and a
        // paragraph long enough to wrap at a typical terminal width. Before
        // the frontmatter-stripping and row-accounting fixes, clicking the
        // "rendering" link landed several rows off because the frontmatter
        // rendered as one giant wrapped heading.
        let knowledge = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/knowledge"));
        let app = App::new(
            knowledge.join("index.md"),
            knowledge,
            Scheme::default_builtin(),
        )
        .unwrap();
        let width = 78u16;

        let rendering_link = app
            .links
            .iter()
            .position(|l| l.target == "rendering/index.md")
            .expect("rendering link present");

        let link = &app.links[rendering_link];
        let (start, end) = markdown::link_col_range(&app.body.lines[link.line], link);
        let click_col = start + (end - start) / 2;
        let click_row = app.row_starts(width)[link.line]; // first row of that line

        let (line, sub_row) = app.line_at_row(click_row, width).unwrap();
        assert_eq!(sub_row, 0, "link's own line should not be pre-wrapped");
        assert_eq!(app.link_at(line, click_col), Some(rendering_link));
    }

    #[test]
    fn line_at_row_finds_wrapped_continuation() {
        let dir =
            std::env::temp_dir().join(format!("term-markdown-rowtest2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.md");
        std::fs::write(&file, "0123456789 abcde\n\nshort\n").unwrap();

        let app = App::new(file, dir.clone(), Scheme::default_builtin()).unwrap();
        let (line, sub_row) = app.line_at_row(0, 10).unwrap();
        assert_eq!((line, sub_row), (0, 0));
        let (line, sub_row) = app.line_at_row(1, 10).unwrap();
        assert_eq!((line, sub_row), (0, 1)); // second wrapped row of line 0

        std::fs::remove_dir_all(&dir).ok();
    }
}
