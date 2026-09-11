mod markdown;

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use markdown::Link;

/// How many terminal rows `text` occupies once word-wrapped at `width`
/// columns, matching ratatui's `Wrap { trim: false }` behavior closely
/// enough to map screen rows back to logical lines (ratatui's own wrapper
/// lives in a private module, so this is a reimplementation, not a call-out
/// to it). A width of 0, or empty text, always occupies exactly one row.
fn wrapped_row_count(text: &str, width: u16) -> u16 {
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
    rows as u16
}

/// Terminal markdown viewer.
#[derive(ClapParser)]
#[command(name = "term-markdown", version, about)]
struct Args {
    /// Markdown file to view
    file: PathBuf,
}

/// Where a link points, resolved relative to the file it appeared in.
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

fn resolve_target(current_file: &Path, target: &str) -> Target {
    if let Some((path_part, fragment)) = target.split_once('#')
        && path_part.is_empty()
    {
        return Target::Anchor(fragment.to_string());
    }
    let path_part = target.split('#').next().unwrap_or(target);
    if path_part.contains("://") || path_part.starts_with("mailto:") {
        return Target::External(target.to_string());
    }
    let base = current_file.parent().unwrap_or_else(|| Path::new("."));
    // Treat a leading '/' as repo/doc-root-relative, not a real filesystem
    // absolute path — otherwise `Path::join` discards `base` entirely and
    // we'd resolve against the host filesystem root instead of the
    // markdown file's own directory.
    let path_part = path_part.trim_start_matches('/');
    let resolved = base.join(path_part);
    if resolved.is_file() {
        Target::File(resolved)
    } else {
        Target::NotFound(resolved)
    }
}

struct App {
    path: PathBuf,
    body: Text<'static>,
    links: Vec<Link>,
    scroll: u16,
    selected_link: Option<usize>,
    history: Vec<(PathBuf, u16)>,
    status: Option<String>,
}

impl App {
    fn new(path: PathBuf) -> Result<Self> {
        let mut app = App {
            path: PathBuf::new(),
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
        let rendered = markdown::render(&source);
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
    /// needs this rather than `self.body.lines.len()`.
    fn row_starts(&self, width: u16) -> Vec<u16> {
        let mut starts = Vec::with_capacity(self.body.lines.len() + 1);
        let mut acc: u16 = 0;
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
    fn line_at_row(&self, row: u16, width: u16) -> Option<(usize, u16)> {
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

    fn max_scroll(&self, viewport_height: u16, width: u16) -> u16 {
        let total = *self.row_starts(width).last().unwrap();
        total.saturating_sub(viewport_height)
    }

    fn scroll_by(&mut self, delta: i32, viewport_height: u16, width: u16) {
        let max = self.max_scroll(viewport_height, width);
        let new = (self.scroll as i32 + delta).clamp(0, max as i32);
        self.scroll = new as u16;
    }

    fn ensure_line_visible(&mut self, line: usize, viewport_height: u16, width: u16) {
        let row = self.row_starts(width)[line];
        if row < self.scroll {
            self.scroll = row;
        } else if viewport_height > 0 && row >= self.scroll + viewport_height {
            self.scroll = row + 1 - viewport_height;
        }
        let max = self.max_scroll(viewport_height, width);
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
        match resolve_target(&self.path, target) {
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
                self.status = Some(format!("link target not found: {}", path.display()));
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
    let mut app = App::new(args.file)?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
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

            app.scroll = app.scroll.min(app.max_scroll(body_height, content_width));

            let mut text = app.body.clone();
            if let Some(idx) = app.selected_link {
                let link = &app.links[idx];
                if let Some(line) = text.lines.get_mut(link.line) {
                    for span in &mut line.spans[link.span_start..link.span_end] {
                        span.style = span.style.add_modifier(Modifier::REVERSED);
                    }
                }
            }

            let block = Block::default()
                .borders(Borders::ALL)
                .title(app.path.display().to_string());
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
                Paragraph::new(Line::from(status_text)).style(Style::default().fg(Color::DarkGray)),
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
                            match app.line_at_row(row, content_width) {
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

    #[test]
    fn resolves_relative_path_to_sibling_file() {
        let dir = std::env::temp_dir().join(format!("term-markdown-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("current.md");
        let sibling = dir.join("other.md");
        std::fs::write(&sibling, "hello").unwrap();

        let resolved = resolve_target(&current, "./other.md");
        match resolved {
            Target::File(path) => assert_eq!(path, sibling),
            _ => panic!("expected File target"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absolute_path_link_resolves_relative_to_current_file_dir() {
        let dir =
            std::env::temp_dir().join(format!("term-markdown-test-abs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("current.md");
        let sibling = dir.join("other.md");
        std::fs::write(&sibling, "hello").unwrap();

        // A link written as an absolute path (e.g. "/other.md") should be
        // treated as relative to current_file's directory, not the host
        // filesystem root.
        let resolved = resolve_target(&current, "/other.md");
        match resolved {
            Target::File(path) => assert_eq!(path, sibling),
            other => panic!("expected File target, got {}", target_kind(other)),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_relative_path_is_not_found() {
        let current = PathBuf::from("/tmp/term-markdown-nonexistent-dir/current.md");
        let resolved = resolve_target(&current, "./missing.md");
        assert_eq!(target_kind(resolved), "not_found");
    }

    #[test]
    fn http_url_is_external() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(&current, "https://example.com");
        assert_eq!(target_kind(resolved), "external");
    }

    #[test]
    fn mailto_is_external() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(&current, "mailto:someone@example.com");
        assert_eq!(target_kind(resolved), "external");
    }

    #[test]
    fn bare_fragment_is_anchor() {
        let current = PathBuf::from("/tmp/current.md");
        let resolved = resolve_target(&current, "#some-heading");
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
    fn row_starts_accounts_for_wrapped_lines() {
        let dir =
            std::env::temp_dir().join(format!("term-markdown-rowtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.md");
        // line 0: wraps to 2 rows at width 10 ("0123456789" + "abcde")
        // line 1: single short row
        std::fs::write(&file, "0123456789 abcde\n\nshort\n").unwrap();

        let app = App::new(file).unwrap();
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
        let app = App::new(PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/knowledge/index.md"
        )))
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

        let app = App::new(file).unwrap();
        let (line, sub_row) = app.line_at_row(0, 10).unwrap();
        assert_eq!((line, sub_row), (0, 0));
        let (line, sub_row) = app.line_at_row(1, 10).unwrap();
        assert_eq!((line, sub_row), (0, 1)); // second wrapped row of line 0

        std::fs::remove_dir_all(&dir).ok();
    }
}
