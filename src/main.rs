mod markdown;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser as ClapParser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

/// Terminal markdown viewer.
#[derive(ClapParser)]
#[command(name = "term-markdown", version, about)]
struct Args {
    /// Markdown file to view
    file: PathBuf,
}

struct App {
    title: String,
    body: Text<'static>,
    scroll: u16,
}

impl App {
    fn new(path: &PathBuf) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Ok(Self {
            title: path.display().to_string(),
            body: markdown::render(&source),
            scroll: 0,
        })
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
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::new(&args.file)?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut body_height: u16 = 0;

    loop {
        terminal.draw(|frame| {
            let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());

            body_height = chunks[0].height.saturating_sub(2);

            let block = Block::default()
                .borders(Borders::ALL)
                .title(app.title.as_str());
            let paragraph = Paragraph::new(app.body.clone())
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll, 0));
            frame.render_widget(paragraph, chunks[0]);

            let max = app.max_scroll(body_height);
            let status = Line::from(format!(
                " {}  |  line {}/{}  |  q: quit  j/k: scroll  g/G: top/bottom",
                app.title,
                app.scroll,
                max
            ));
            frame.render_widget(
                Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
                chunks[1],
            );
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
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
                    KeyCode::Char('g') | KeyCode::Home => app.scroll_by(i32::MIN / 2, body_height),
                    KeyCode::Char('G') | KeyCode::End => app.scroll_by(i32::MAX / 2, body_height),
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
