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

use markdown::Link;

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

    fn max_scroll(&self, viewport_height: u16) -> u16 {
        let total = self.body.lines.len() as u16;
        total.saturating_sub(viewport_height)
    }

    fn scroll_by(&mut self, delta: i32, viewport_height: u16) {
        let max = self.max_scroll(viewport_height);
        let new = (self.scroll as i32 + delta).clamp(0, max as i32);
        self.scroll = new as u16;
    }

    fn ensure_line_visible(&mut self, line: usize, viewport_height: u16) {
        let line = line as u16;
        if line < self.scroll {
            self.scroll = line;
        } else if viewport_height > 0 && line >= self.scroll + viewport_height {
            self.scroll = line + 1 - viewport_height;
        }
        let max = self.max_scroll(viewport_height);
        self.scroll = self.scroll.min(max);
    }

    fn select_next_link(&mut self, forward: bool, viewport_height: u16) {
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
        self.ensure_line_visible(line, viewport_height);
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

    loop {
        terminal.draw(|frame| {
            let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());

            body_area = chunks[0];
            body_height = chunks[0].height.saturating_sub(2);

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

            let max = app.max_scroll(body_height);
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
                        KeyCode::Char('j') | KeyCode::Down => app.scroll_by(1, body_height),
                        KeyCode::Char('k') | KeyCode::Up => app.scroll_by(-1, body_height),
                        KeyCode::Char('d') | KeyCode::PageDown => {
                            app.scroll_by(body_height as i32 / 2, body_height)
                        }
                        KeyCode::Char('u') | KeyCode::PageUp => {
                            app.scroll_by(-(body_height as i32) / 2, body_height)
                        }
                        KeyCode::Char('g') | KeyCode::Home => {
                            app.scroll_by(i32::MIN / 2, body_height)
                        }
                        KeyCode::Char('G') | KeyCode::End => {
                            app.scroll_by(i32::MAX / 2, body_height)
                        }
                        KeyCode::Tab => app.select_next_link(true, body_height),
                        KeyCode::BackTab => app.select_next_link(false, body_height),
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
                            let line = app.scroll as usize + (mouse.row - body_area.y - 1) as usize;
                            let col = mouse.column - body_area.x - 1;
                            if let Some(idx) = app.link_at(line, col) {
                                app.status = None;
                                app.selected_link = Some(idx);
                                app.follow_selected();
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
}
