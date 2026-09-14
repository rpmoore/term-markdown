---
type: concept
title: Markdown rendering pipeline
description: How markdown becomes a styled ratatui Text, with fenced-code syntax highlighting.
resource: src/markdown.rs
tags: [rendering, ratatui, pulldown-cmark, syntect]
---

# Markdown rendering pipeline

`render(source: &str, scheme: &Scheme) -> Rendered` (`src/markdown.rs:122`) is the sole entry
point. Every color/style used for markdown elements — heading colors, blockquote, code-fence
markers, code-block background, inline code, list markers, links, image alt text, table-header
bold, horizontal rule — is read from `scheme.markdown` (see
[scheme-loading](../config/scheme-loading.md) for how `Scheme` is built); `render` itself has no
hardcoded colors left. This runs on every call, including navigation between files, since
`Scheme` is loaded once at startup and reused. It first calls `strip_frontmatter`
(`src/markdown.rs`) to drop a leading YAML frontmatter block (`---` ... `---`) before parsing:
pulldown-cmark has no frontmatter concept, so left in, a closing `---` right after non-blank
lines reads as a Setext-heading underline and the entire frontmatter block becomes one giant
heading `Line`. `strip_frontmatter` only acts when the file starts with `---\n` *and* a closing
`\n---\n` (or `\n---` at EOF) is found later; a bare leading `---` with no closing fence is left
untouched (it's a thematic break, not frontmatter). Every concept doc under `docs/knowledge/`
(e.g. this file) carries frontmatter, so this runs on every concept-doc render — the area
`index.md` files have none (OKF spec §8), so there's nothing for it to strip there. It runs a
single pass over `pulldown_cmark::Parser` events and builds a `Vec<Line<'static>>`, returned as
`Text::from(lines)` (`src/markdown.rs:359`). `pulldown_cmark::Parser::new_ext` is called with
`Options::ENABLE_TABLES` (`src/markdown.rs:148`) — the only non-default parser option — so pipe
tables (`| a | b |`) parse as real table events instead of plain paragraph text; see Tables below.

## Style stack

A `Vec<Style>` (`src/markdown.rs:130`) tracks nested inline/block styling. Each `Event::Start`
that carries styling (heading, emphasis, strong, strikethrough, blockquote, link, image, table
cell) pushes a derived `Style` onto the stack; the matching `Event::End` pops it. Plain text
(`Event::Text`) is pushed as a `Span` styled with `*style_stack.last().unwrap()` — the stack
must never be popped empty, since every push has a matching pop keyed to the same tag. Elements
that carry a scheme color/modifier combine it with the inherited style via `Style::patch` (e.g.
`style.patch(scheme.markdown.heading_h1)`, `src/markdown.rs:158`) rather than replacing it
outright, so a colored element nested inside another (e.g. a link inside emphasis) keeps both.

Heading level maps to color only, not depth-dependent indent: H1 → `scheme.markdown.heading_h1`,
H2 → `scheme.markdown.heading_h2`, H3+ → `scheme.markdown.heading_h3`
(`src/markdown.rs:152-158`). The built-in default scheme reproduces the original hardcoded
values (yellow/cyan/magenta, bold).

## Line buffering

`current: Vec<Span>` accumulates spans for the line in progress; `flush_line`
(`src/markdown.rs:82`) moves it into `lines` via `mem::take`. Paragraphs, headings, list items,
and blockquotes each flush on their `End` event and (for paragraph/heading) push a blank
`Line::from("")` afterward for spacing.

## Lists

`list_stack: Vec<Option<u64>>` (`src/markdown.rs:131`) holds one entry per nesting level:
`Some(n)` for an ordered list's next-number counter, `None` for unordered. `Tag::Item` computes
indent from stack depth (`"  ".repeat(depth)`) and either renders `"{n}. "` and increments the
counter, or `"• "`, styled with `scheme.markdown.list_marker`.

## Fenced code blocks

Inline code (single backtick, `Event::Code`, `src/markdown.rs:313`) is rendered directly as a
span styled with `scheme.markdown.code_inline` — it does not go through syntect.

Fenced/indented code blocks are buffered, not streamed: `Tag::CodeBlock` sets `in_code_block =
true` and clears `code_buffer`/`code_lang`; every `Event::Text`, `SoftBreak`, and `HardBreak`
while `in_code_block` appends raw text/newlines to `code_buffer` instead of touching
`current`/`lines`. On `TagEnd::CodeBlock` the whole buffer is highlighted at once via
`highlight_code_block`.

`highlight_code_block` (`src/markdown.rs:34-63`) resolves a `syntect::SyntaxReference` by fence
language token (`find_syntax_by_token`, falling back to `find_syntax_plain_text` when the token
is empty or unrecognized), then runs `HighlightLines` per source line (`LinesWithEndings::from`,
which preserves newlines as syntect's regexes expect). The `syntect::highlighting::Theme` used
is `&scheme.syntax_theme` — resolved once when the `Scheme` is loaded (see
[scheme-loading](../config/scheme-loading.md)), not loaded inside
`render`/`highlight_code_block` itself. Each `(SynStyle, &str)` range is converted via
`syn_style_to_ratatui` (`src/markdown.rs:16`, maps
`syntect::highlighting::Color`→`ratatui::style::Color::Rgb` and bold/italic/underline
`FontStyle` bits→`Modifier`) and patched with the scheme's `code_block_bg` style, passed into
`highlight_code_block` as the `bg: Style` parameter, so highlighted spans still have a
code-block backdrop.

Fence lines (`` ```lang `` / `` ``` ``) are emitted as `Line`s styled with
`scheme.markdown.code_fence_marker`, not passed through syntect.

## Link tracking

`render` returns `Rendered { text, links }` (`src/markdown.rs:77`), not a bare `Text` — `links:
Vec<Link>` records every markdown link found, each located by `(line, span_start, span_end)`
into the returned `Text`'s `Line::spans`, not by screen column: column position depends on
wrapping/scroll, so it's cheap to recompute on demand instead (see `link_col_range`,
`src/markdown.rs:367`, used only for mouse hit-testing).

`Tag::Link { dest_url, .. }` pushes `style.patch(scheme.markdown.link)` and `(current.len(),
dest_url)` onto `link_stack` (`src/markdown.rs:215-217`); `TagEnd::Link` pops it, and if the
link's text produced at least one span (`span_end > span_start`), stages `(span_start, span_end,
target)` into `pending_links` (`src/markdown.rs:275-283`). Staged links resolve to a concrete
line index only at the next `flush_line` call (`src/markdown.rs:82-98`), since a link's owning
`current`/`Line` may not be flushed until later in the same block (e.g. more text after the
link, before the paragraph ends) — `pending_links` is drained into `links` at that flush,
stamped with `lines.len()` (the index the flushed line is about to occupy). This assumes a
link's start and end always fall within one `flush_line`-delimited segment (true today: nothing
flushes mid-link since link contents are inline-only, no block-level breaks).

Images (`Tag::Image`, `src/markdown.rs:219`) are styled with `scheme.markdown.image_alt` but not
tracked as links — not navigable targets for a file viewer.

## Tables

`Tag::TableHead` starts the header row and `Tag::TableRow` each body row
(`src/markdown.rs:220-231`); `Tag::TableCell` pushes a two-space separator between cells within a
row (`TagEnd::TableCell`, `src/markdown.rs:296-299`) but, unlike `TableHead`/`TableRow`, does
*not* flush the line — cells accumulate into the same `current` line until the row itself ends.
`TagEnd::TableHead` and `TagEnd::TableRow` each call `flush_line` (`src/markdown.rs:287-295`), so
the header and every body row land on their own `Line`; `TagEnd::Table` adds a trailing blank line
for spacing (`src/markdown.rs:300-302`), matching other block elements. Without this per-row
flush, an entire table (header plus every body row) would accumulate into one unbroken `current`
buffer and wrap as a single unreadable blob — this was caught by rendering this crate's own
`app-loop.md`/`README.md` key-binding tables after enabling table parsing (see Known gap below)
and confirmed fixed by inspecting real terminal output.

Since `table_header_bold` names a *header* setting, only header cells are bolded: an
`in_table_head` flag is set on `Tag::TableHead` and cleared on `TagEnd::TableHead`
(`src/markdown.rs:221, 288`), and `Tag::TableCell` only applies `Modifier::BOLD` when both
`in_table_head` and `scheme.markdown.table_header_bold` are true (`src/markdown.rs:225-231`) —
body-row cells never get the header style. Covered by
`markdown::tests::table_header_row_is_bold_and_separate_from_body_rows` and
`table_header_bold_disabled_leaves_header_unbolded`.

## Known gap

Tables still don't align columns or draw borders — cells within a row are just
two-space-separated in reading order, and a row wider than the terminal wraps like any other long
line, with no re-indent to keep later columns lined up under earlier rows. `Tag::Table`'s column
alignments (`Tag::Table(alignments)`) are ignored entirely (falls into the catch-all `_ => {}`
arm). Fixing column alignment would need a two-pass approach (measure all cell widths in a table
before emitting any row) that the current single-pass event loop doesn't support; out of scope for
the color-scheme work that enabled table parsing in the first place.
