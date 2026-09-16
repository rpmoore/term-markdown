use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SynColor, FontStyle, Style as SynStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthStr;

use crate::scheme::Scheme;

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
    bg: Style,
    lines: &mut Vec<Line<'static>>,
) {
    let syntax: &SyntaxReference = syntax_set
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);

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

/// A link found while rendering, located by the `Line`/`Span` range it
/// occupies in the returned `Text` (rather than by screen column, since
/// column position depends on wrapping/scroll and is cheap to recompute
/// on demand from the spans when actually needed for hit-testing).
#[derive(Debug, Clone)]
pub struct Link {
    pub line: usize,
    pub span_start: usize,
    pub span_end: usize,
    pub target: String,
}

pub struct Rendered {
    pub text: Text<'static>,
    pub links: Vec<Link>,
}

fn flush_line(
    current: &mut Vec<Span<'static>>,
    lines: &mut Vec<Line<'static>>,
    pending_links: &mut Vec<(usize, usize, String)>,
    links: &mut Vec<Link>,
) {
    let line = lines.len();
    for (span_start, span_end, target) in pending_links.drain(..) {
        links.push(Link {
            line,
            span_start,
            span_end,
            target,
        });
    }
    lines.push(Line::from(std::mem::take(current)));
}

/// Strip a leading YAML frontmatter block (`---` ... `---`), if present.
/// pulldown-cmark has no frontmatter concept: left in, a `---` closing fence
/// right after non-blank lines reads as a Setext heading underline, turning
/// the whole frontmatter block into one giant heading line.
fn strip_frontmatter(source: &str) -> &str {
    let Some(rest) = source.strip_prefix("---\n") else {
        return source;
    };
    if let Some(end) = rest.find("\n---\n") {
        // Closing fence found mid-file: body starts after it.
        return &rest[end + 5..];
    }
    if rest.strip_suffix("\n---\n").is_some() || rest.strip_suffix("\n---").is_some() {
        // The whole file is frontmatter.
        return "";
    }
    // No closing fence: not actually frontmatter, leave untouched.
    source
}

/// Convert a markdown source string into a styled ratatui `Text` (plus the
/// links found in it) ready for display in a scrollable widget.
pub fn render(source: &str, scheme: &Scheme) -> Rendered {
    let source = strip_frontmatter(source);
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme = &scheme.syntax_theme;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();

    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    let mut in_code_block = false;
    let mut in_table_head = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    let mut links: Vec<Link> = Vec::new();
    let mut pending_links: Vec<(usize, usize, String)> = Vec::new();
    let mut link_stack: Vec<(usize, String)> = Vec::new();

    let push_span = |current: &mut Vec<Span<'static>>, text: String, style: Style| {
        if !text.is_empty() {
            current.push(Span::styled(text, style));
        }
    };

    for event in Parser::new_ext(source, Options::ENABLE_TABLES) {
        let style = *style_stack.last().unwrap();
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let heading_style = match level {
                        HeadingLevel::H1 => scheme.markdown.heading_h1,
                        HeadingLevel::H2 => scheme.markdown.heading_h2,
                        _ => scheme.markdown.heading_h3,
                    };
                    style_stack.push(style.patch(heading_style));
                    let prefix = "#".repeat(level as usize) + " ";
                    push_span(&mut current, prefix, *style_stack.last().unwrap());
                }
                Tag::Paragraph => {}
                Tag::Emphasis => style_stack.push(style.add_modifier(Modifier::ITALIC)),
                Tag::Strong => style_stack.push(style.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => style_stack.push(style.add_modifier(Modifier::CROSSED_OUT)),
                Tag::BlockQuote(_) => {
                    style_stack.push(style.patch(scheme.markdown.blockquote));
                    push_span(
                        &mut current,
                        scheme.markdown.blockquote_marker.clone(),
                        *style_stack.last().unwrap(),
                    );
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    code_buffer.clear();
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    if !current.is_empty() {
                        flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                    }
                    lines.push(Line::from(""));
                    if !code_lang.is_empty() {
                        lines.push(Line::from(Span::styled(
                            format!("  ```{code_lang}"),
                            scheme.markdown.code_fence_marker,
                        )));
                    } else {
                        lines.push(Line::from(Span::styled(
                            "  ```",
                            scheme.markdown.code_fence_marker,
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
                        style.patch(scheme.markdown.list_marker),
                    );
                }
                Tag::Link { dest_url, .. } => {
                    style_stack.push(style.patch(scheme.markdown.link));
                    link_stack.push((current.len(), dest_url.to_string()));
                }
                Tag::Image { .. } => style_stack.push(style.patch(scheme.markdown.image_alt)),
                Tag::TableHead => {
                    in_table_head = true;
                    style_stack.push(style);
                }
                Tag::TableRow => style_stack.push(style),
                Tag::TableCell => {
                    let mut cell_style = style;
                    if in_table_head && scheme.markdown.table_header_bold {
                        cell_style = cell_style.add_modifier(Modifier::BOLD);
                    }
                    style_stack.push(cell_style);
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                    lines.push(Line::from(""));
                }
                TagEnd::Paragraph => {
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                    lines.push(Line::from(""));
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    style_stack.pop();
                }
                TagEnd::BlockQuote(_) => {
                    style_stack.pop();
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    highlight_code_block(
                        &code_buffer,
                        &code_lang,
                        &syntax_set,
                        theme,
                        scheme.markdown.code_block_bg,
                        &mut lines,
                    );
                    code_buffer.clear();
                    lines.push(Line::from(Span::styled(
                        "  ```",
                        scheme.markdown.code_fence_marker,
                    )));
                    lines.push(Line::from(""));
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    lines.push(Line::from(""));
                }
                TagEnd::Item => {
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
                TagEnd::Link => {
                    style_stack.pop();
                    if let Some((span_start, target)) = link_stack.pop() {
                        let span_end = current.len();
                        if span_end > span_start {
                            pending_links.push((span_start, span_end, target));
                        }
                    }
                }
                TagEnd::Image => {
                    style_stack.pop();
                }
                TagEnd::TableHead => {
                    in_table_head = false;
                    style_stack.pop();
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
                TagEnd::TableRow => {
                    style_stack.pop();
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
                TagEnd::TableCell => {
                    style_stack.pop();
                    push_span(&mut current, "  ".to_string(), style);
                }
                TagEnd::Table => {
                    lines.push(Line::from(""));
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
                    scheme.markdown.code_inline,
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
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
            }
            Event::Rule => {
                if !current.is_empty() {
                    flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
                }
                lines.push(Line::from(Span::styled(
                    scheme
                        .markdown
                        .horizontal_rule_glyph
                        .repeat(scheme.markdown.horizontal_rule_width),
                    scheme.markdown.horizontal_rule,
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
        flush_line(&mut current, &mut lines, &mut pending_links, &mut links);
    }

    Rendered {
        text: Text::from(lines),
        links,
    }
}

/// The `[start, end)` terminal-column range a link occupies on its line,
/// computed on demand (rather than stored during `render`) since it's only
/// needed for mouse hit-testing.
pub fn link_col_range(line: &Line<'_>, link: &Link) -> (u16, u16) {
    let start: usize = line.spans[..link.span_start]
        .iter()
        .map(|s| s.content.width())
        .sum();
    let width: usize = line.spans[link.span_start..link.span_end]
        .iter()
        .map(|s| s.content.width())
        .sum();
    (start as u16, (start + width) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders with the built-in default scheme, for tests that only care
    /// about text/link structure, not styling.
    fn render_default(source: &str) -> Rendered {
        render(source, &Scheme::default_builtin())
    }

    #[test]
    fn extracts_link_target_and_text() {
        let rendered = render_default("see [the docs](./other.md) for more");
        assert_eq!(rendered.links.len(), 1);
        let link = &rendered.links[0];
        assert_eq!(link.target, "./other.md");

        let line = &rendered.text.lines[link.line];
        let text: String = line.spans[link.span_start..link.span_end]
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "the docs");
    }

    #[test]
    fn link_col_range_matches_preceding_text_width() {
        let rendered = render_default("abc [x](y)");
        let link = &rendered.links[0];
        let line = &rendered.text.lines[link.line];
        let (start, end) = link_col_range(line, link);
        assert_eq!(start, 4); // "abc " is 4 columns wide
        assert_eq!(end, 5); // "x" is 1 column wide
    }

    #[test]
    fn plain_text_has_no_links() {
        let rendered = render_default("just plain text, no links here");
        assert!(rendered.links.is_empty());
    }

    #[test]
    fn fenced_code_block_is_highlighted_without_touching_links() {
        let rendered = render_default("```rust\nfn main() {}\n```");
        assert!(rendered.links.is_empty());
        // fence lines + at least one highlighted body line should be present
        assert!(rendered.text.lines.len() >= 3);
    }

    #[test]
    fn frontmatter_is_stripped_before_parsing() {
        let src = "---\ntitle: hi\ntags: [a, b]\n---\n\n# Heading\n\nbody text\n";
        let rendered = render_default(src);
        let first_line: String = rendered.text.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(first_line, "# Heading");
    }

    #[test]
    fn frontmatter_only_file_renders_empty() {
        let rendered = render_default("---\ntitle: hi\n---\n");
        assert!(rendered.text.lines.is_empty() || rendered.text.lines == vec![Line::from("")]);
    }

    #[test]
    fn no_frontmatter_is_left_untouched() {
        let rendered = render_default("# Heading\n\nbody\n");
        let first_line: String = rendered.text.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(first_line, "# Heading");
    }

    #[test]
    fn dashes_without_closing_fence_are_not_treated_as_frontmatter() {
        // "---" alone at the top with no second "---" is a thematic break,
        // not frontmatter - must not be swallowed.
        let rendered = render_default("---\nnot frontmatter, just a rule above this text\n");
        let joined: String = rendered
            .text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("not frontmatter"));
    }

    // The following tests assert on `Span.style`/`Color` values directly —
    // proving a scheme's colors actually flow into rendering, not just that
    // `default_builtin()` happens to match the old hardcoded literals.
    // Each uses a distinguishable, arbitrary color/glyph not shared with
    // `default_builtin()`.

    #[test]
    fn heading_h1_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.heading_h1 = Style::default().fg(Color::Rgb(0x12, 0x34, 0x56));
        let rendered = render("# Title", &scheme);
        assert_eq!(
            rendered.text.lines[0].spans[0].style.fg,
            Some(Color::Rgb(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn heading_h2_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.heading_h2 = Style::default().fg(Color::Rgb(0x22, 0x33, 0x44));
        let rendered = render("## Title", &scheme);
        assert_eq!(
            rendered.text.lines[0].spans[0].style.fg,
            Some(Color::Rgb(0x22, 0x33, 0x44))
        );
    }

    #[test]
    fn heading_h3_and_deeper_use_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.heading_h3 = Style::default().fg(Color::Rgb(0x55, 0x66, 0x77));
        let h3 = render("### Title", &scheme);
        let h6 = render("###### Title", &scheme);
        assert_eq!(
            h3.text.lines[0].spans[0].style.fg,
            Some(Color::Rgb(0x55, 0x66, 0x77))
        );
        assert_eq!(
            h6.text.lines[0].spans[0].style.fg,
            Some(Color::Rgb(0x55, 0x66, 0x77))
        );
    }

    #[test]
    fn blockquote_uses_scheme_style_and_marker() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.blockquote = Style::default().fg(Color::Rgb(1, 2, 3));
        scheme.markdown.blockquote_marker = ">> ".to_string();
        let rendered = render("> quoted", &scheme);
        let line = &rendered.text.lines[0];
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with(">> "));
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn code_fence_marker_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.code_fence_marker = Style::default().fg(Color::Rgb(9, 9, 9));
        let rendered = render("```rust\nfn x() {}\n```", &scheme);
        let fence_line = rendered
            .text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("```rust")))
            .expect("fence line present");
        assert_eq!(fence_line.spans[0].style.fg, Some(Color::Rgb(9, 9, 9)));
    }

    #[test]
    fn code_block_bg_uses_scheme_color() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.code_block_bg = Style::default().bg(Color::Rgb(4, 5, 6));
        let rendered = render("```\nhello\n```", &scheme);
        let body_line = rendered
            .text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("hello")))
            .expect("code body line present");
        assert_eq!(body_line.spans[0].style.bg, Some(Color::Rgb(4, 5, 6)));
    }

    #[test]
    fn inline_code_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.code_inline = Style::default()
            .fg(Color::Rgb(7, 7, 7))
            .bg(Color::Rgb(8, 8, 8));
        let rendered = render("use `code` here", &scheme);
        let line = &rendered.text.lines[0];
        let span = line
            .spans
            .iter()
            .find(|s| s.content.contains("code"))
            .expect("code span present");
        assert_eq!(span.style.fg, Some(Color::Rgb(7, 7, 7)));
        assert_eq!(span.style.bg, Some(Color::Rgb(8, 8, 8)));
    }

    #[test]
    fn list_marker_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.list_marker = Style::default().fg(Color::Rgb(10, 20, 30));
        let rendered = render("- item", &scheme);
        let line = &rendered.text.lines[0];
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn link_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.link = Style::default().fg(Color::Rgb(11, 22, 33));
        let rendered = render("[text](url)", &scheme);
        let link = &rendered.links[0];
        let line = &rendered.text.lines[link.line];
        assert_eq!(
            line.spans[link.span_start].style.fg,
            Some(Color::Rgb(11, 22, 33))
        );
    }

    #[test]
    fn image_alt_uses_scheme_style() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.image_alt = Style::default().fg(Color::Rgb(44, 55, 66));
        let rendered = render("![alt text](pic.png)", &scheme);
        let line = &rendered.text.lines[0];
        let span = line
            .spans
            .iter()
            .find(|s| s.content.contains("alt"))
            .expect("alt span present");
        assert_eq!(span.style.fg, Some(Color::Rgb(44, 55, 66)));
    }

    #[test]
    fn horizontal_rule_uses_scheme_style_and_glyph() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.horizontal_rule = Style::default().fg(Color::Rgb(77, 88, 99));
        scheme.markdown.horizontal_rule_glyph = "=".to_string();
        scheme.markdown.horizontal_rule_width = 5;
        let rendered = render("before\n\n---\n\nafter", &scheme);
        let rule_line = rendered
            .text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('=')))
            .expect("rule line present");
        let text: String = rule_line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "=====");
        assert_eq!(rule_line.spans[0].style.fg, Some(Color::Rgb(77, 88, 99)));
    }

    #[test]
    fn syntect_theme_affects_code_block_colors() {
        let ocean = Scheme::default_builtin();
        let mut solarized = Scheme::default_builtin();
        solarized.syntax_theme = syntect::highlighting::ThemeSet::load_defaults()
            .themes
            .get("Solarized (dark)")
            .expect("bundled \"Solarized (dark)\" theme available")
            .clone();

        let src = "```rust\nfn main() {}\n```";
        let ocean_colors: Vec<_> = render(src, &ocean)
            .text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.style.fg)
            .collect();
        let solarized_colors: Vec<_> = render(src, &solarized)
            .text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.style.fg)
            .collect();
        assert_ne!(ocean_colors, solarized_colors);
    }

    #[test]
    fn table_header_row_is_bold_and_separate_from_body_rows() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.table_header_bold = true;
        let src = "| Key | Action |\n|---|---|\n| q | quit |\n| j | scroll |\n";
        let rendered = render(src, &scheme);

        let line_text =
            |line: &Line<'_>| -> String { line.spans.iter().map(|s| s.content.as_ref()).collect() };
        let texts: Vec<String> = rendered.text.lines.iter().map(line_text).collect();
        // Exactly: header row, each body row, one trailing blank line after the
        // table — no spurious blank line between the header and first row.
        assert_eq!(texts, vec!["Key  Action  ", "q  quit  ", "j  scroll  ", ""]);

        let header_line = &rendered.text.lines[0];
        let body_line = &rendered.text.lines[1];

        // Header and first body row must be on separate lines, not run together.
        assert!(!line_text(header_line).contains("quit"));
        assert!(
            header_line
                .spans
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            body_line
                .spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn table_header_bold_disabled_leaves_header_unbolded() {
        let mut scheme = Scheme::default_builtin();
        scheme.markdown.table_header_bold = false;
        let src = "| Key | Action |\n|---|---|\n| q | quit |\n";
        let rendered = render(src, &scheme);

        let header_line = rendered
            .text
            .lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("Key")
            })
            .expect("header line present");
        assert!(
            header_line
                .spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::BOLD))
        );
    }
}
