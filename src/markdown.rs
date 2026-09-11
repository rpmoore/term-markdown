use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, FontStyle, Style as SynStyle, Theme, ThemeSet,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

fn syn_color_to_ratatui(c: SynColor) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

fn syn_style_to_ratatui(style: SynStyle) -> Style {
    let mut modifier = Modifier::empty();
    if style.font_style.contains(FontStyle::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        modifier |= Modifier::UNDERLINED;
    }
    Style::default()
        .fg(syn_color_to_ratatui(style.foreground))
        .add_modifier(modifier)
}

/// Syntax-highlight a fenced code block's contents and append it (indented)
/// as individual lines.
fn highlight_code_block(
    code: &str,
    lang: &str,
    syntax_set: &SyntaxSet,
    theme: &Theme,
    lines: &mut Vec<Line<'static>>,
) {
    let syntax: &SyntaxReference = syntax_set
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let bg = Style::default().bg(Color::Rgb(30, 30, 30));

    for src_line in LinesWithEndings::from(code) {
        let ranges = highlighter
            .highlight_line(src_line, syntax_set)
            .unwrap_or_default();

        let mut spans: Vec<Span<'static>> = vec![Span::styled("  ", bg)];
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']).to_string();
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text, syn_style_to_ratatui(style).patch(bg)));
        }
        lines.push(Line::from(spans));
    }
}

/// Convert a markdown source string into a styled ratatui `Text` ready for
/// display in a scrollable widget.
pub fn render(source: &str) -> Text<'static> {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["base16-ocean.dark"];

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();

    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    let flush_line = |current: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>| {
        lines.push(Line::from(std::mem::take(current)));
    };

    let push_span = |current: &mut Vec<Span<'static>>, text: String, style: Style| {
        if !text.is_empty() {
            current.push(Span::styled(text, style));
        }
    };

    for event in Parser::new(source) {
        let style = *style_stack.last().unwrap();
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let color = match level {
                        HeadingLevel::H1 => Color::Yellow,
                        HeadingLevel::H2 => Color::Cyan,
                        _ => Color::Magenta,
                    };
                    style_stack.push(style.fg(color).add_modifier(Modifier::BOLD));
                    let prefix = "#".repeat(level as usize) + " ";
                    push_span(&mut current, prefix, *style_stack.last().unwrap());
                }
                Tag::Paragraph => {}
                Tag::Emphasis => style_stack.push(style.add_modifier(Modifier::ITALIC)),
                Tag::Strong => style_stack.push(style.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => style_stack.push(style.add_modifier(Modifier::CROSSED_OUT)),
                Tag::BlockQuote(_) => {
                    style_stack.push(style.fg(Color::DarkGray).add_modifier(Modifier::ITALIC));
                    push_span(&mut current, "\u{2503} ".to_string(), *style_stack.last().unwrap());
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    code_buffer.clear();
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    if !current.is_empty() {
                        flush_line(&mut current, &mut lines);
                    }
                    lines.push(Line::from(""));
                    if !code_lang.is_empty() {
                        lines.push(Line::from(Span::styled(
                            format!("  ```{code_lang}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    } else {
                        lines.push(Line::from(Span::styled(
                            "  ```",
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                Tag::List(start) => list_stack.push(start),
                Tag::Item => {
                    let depth = list_stack.len().saturating_sub(1);
                    let indent = "  ".repeat(depth);
                    let marker = match list_stack.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "\u{2022} ".to_string(),
                    };
                    push_span(
                        &mut current,
                        format!("{indent}{marker}"),
                        style.fg(Color::White),
                    );
                }
                Tag::Link { .. } => {
                    style_stack.push(style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED))
                }
                Tag::Image { .. } => style_stack.push(style.fg(Color::Magenta)),
                Tag::TableHead | Tag::TableRow | Tag::TableCell => {
                    style_stack.push(style.add_modifier(Modifier::BOLD));
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush_line(&mut current, &mut lines);
                    lines.push(Line::from(""));
                }
                TagEnd::Paragraph => {
                    flush_line(&mut current, &mut lines);
                    lines.push(Line::from(""));
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    style_stack.pop();
                }
                TagEnd::BlockQuote(_) => {
                    style_stack.pop();
                    flush_line(&mut current, &mut lines);
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    highlight_code_block(&code_buffer, &code_lang, &syntax_set, theme, &mut lines);
                    code_buffer.clear();
                    lines.push(Line::from(Span::styled(
                        "  ```",
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(""));
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    lines.push(Line::from(""));
                }
                TagEnd::Item => {
                    flush_line(&mut current, &mut lines);
                }
                TagEnd::Link | TagEnd::Image => {
                    style_stack.pop();
                }
                TagEnd::TableHead | TagEnd::TableRow | TagEnd::TableCell => {
                    style_stack.pop();
                    push_span(&mut current, "  ".to_string(), style);
                }
                _ => {}
            },
            Event::Text(text) => {
                let s = text.into_string();
                if in_code_block {
                    code_buffer.push_str(&s);
                } else {
                    push_span(&mut current, s, style);
                }
            }
            Event::Code(text) => {
                push_span(
                    &mut current,
                    format!(" {} ", text.into_string()),
                    Style::default().fg(Color::Green).bg(Color::Rgb(40, 40, 40)),
                );
            }
            Event::SoftBreak => {
                if in_code_block {
                    code_buffer.push('\n');
                } else {
                    push_span(&mut current, " ".to_string(), style);
                }
            }
            Event::HardBreak => {
                if in_code_block {
                    code_buffer.push('\n');
                } else {
                    flush_line(&mut current, &mut lines);
                }
            }
            Event::Rule => {
                if !current.is_empty() {
                    flush_line(&mut current, &mut lines);
                }
                lines.push(Line::from(Span::styled(
                    "\u{2500}".repeat(60),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                push_span(&mut current, mark.to_string(), style);
            }
            _ => {}
        }
    }

    if !current.is_empty() {
        flush_line(&mut current, &mut lines);
    }

    Text::from(lines)
}
