---
type: concept
title: Markdown rendering pipeline
description: How markdown source becomes a styled ratatui Text, including fenced-code syntax highlighting.
resource: src/markdown.rs
tags: [rendering, ratatui, pulldown-cmark, syntect]
---

# Markdown rendering pipeline

`render(source: &str) -> Text<'static>` (`src/markdown.rs:66`) is the sole entry point. It runs a single pass over `pulldown_cmark::Parser` events and builds a `Vec<Line<'static>>`, returned as `Text::from(lines)` (`src/markdown.rs:256`).

## Style stack

A `Vec<Style>` (`src/markdown.rs:74`) tracks nested inline/block styling. Each `Event::Start` that carries styling (heading, emphasis, strong, strikethrough, blockquote, link, image, table cell) pushes a derived `Style` onto the stack (`src/markdown.rs:94-161`); the matching `Event::End` pops it (`src/markdown.rs:163-205`). Plain text (`Event::Text`) is pushed as a `Span` styled with `*style_stack.last().unwrap()` (`src/markdown.rs:92`, `src/markdown.rs:211`) — the stack must never be popped empty, since every push has a matching pop keyed to the same tag.

Heading level maps to color only, not depth-dependent indent: H1 → yellow, H2 → cyan, H3+ → magenta, all bold (`src/markdown.rs:96-100`).

## Line buffering

`current: Vec<Span>` accumulates spans for the line in progress; `flush_line` (`src/markdown.rs:81-83`) moves it into `lines` via `mem::take`. Paragraphs, headings, list items, and blockquotes each flush on their `End` event and (for paragraph/heading) push a blank `Line::from("")` afterward for spacing (`src/markdown.rs:166-171`).

## Lists

`list_stack: Vec<Option<u64>>` (`src/markdown.rs:75`) holds one entry per nesting level: `Some(n)` for an ordered list's next-number counter, `None` for unordered. `Tag::Item` (`src/markdown.rs:137-153`) computes indent from stack depth (`"  ".repeat(depth)`) and either renders `"{n}. "` and increments the counter, or `"• "`.

## Fenced code blocks

Inline code (single backtick, `Event::Code`) is rendered directly as a fixed green-on-dark span (`src/markdown.rs:214-219`) — it does not go through syntect.

Fenced/indented code blocks are buffered, not streamed: `Tag::CodeBlock` sets `in_code_block = true` and clears `code_buffer`/`code_lang` (`src/markdown.rs:113-119`); every `Event::Text`, `SoftBreak`, and `HardBreak` while `in_code_block` appends raw text/newlines to `code_buffer` instead of touching `current`/`lines` (`src/markdown.rs:206-213`, `221-234`). On `TagEnd::CodeBlock` the whole buffer is highlighted at once via `highlight_code_block` (`src/markdown.rs:180-183`).

`highlight_code_block` (`src/markdown.rs:33-62`) resolves a `syntect::SyntaxReference` by fence language token (`find_syntax_by_token`, falling back to `find_syntax_plain_text` when the token is empty or unrecognized — `src/markdown.rs:40-42`), then runs `HighlightLines` per source line (`LinesWithEndings::from`, which preserves newlines as syntect's regexes expect — `src/markdown.rs:47`). Each `(SynStyle, &str)` range is converted via `syn_style_to_ratatui` (`src/markdown.rs:15-29`, maps `syntect::highlighting::Color`→`ratatui::style::Color::Rgb` and bold/italic/underline `FontStyle` bits→`Modifier`) and patched with a shared dark background (`Color::Rgb(30,30,30)`, `src/markdown.rs:45,58`) so highlighted spans still have a code-block backdrop. The syntax/theme sets (`SyntaxSet::load_defaults_newlines`, `ThemeSet::load_defaults`, theme `"base16-ocean.dark"` — `src/markdown.rs:67-69`) are loaded once per `render()` call, not cached across calls.

Fence lines (`` ```lang `` / `` ``` ``) are emitted as plain gray `Line`s surrounding the highlighted body, not passed through syntect (`src/markdown.rs:124-134`, `184-187`).

## Link tracking

`render` returns `Rendered { text, links }` (`src/markdown.rs:75-78`), not a bare `Text` — `links: Vec<Link>` records every markdown link found, each located by `(line, span_start, span_end)` into the returned `Text`'s `Line::spans`, not by screen column: column position depends on wrapping/scroll, so it's cheap to recompute on demand instead (see `link_col_range`, `src/markdown.rs`, used only for mouse hit-testing).

`Tag::Link { dest_url, .. }` pushes `(current.len(), dest_url)` onto `link_stack` (`src/markdown.rs:192-195`); `TagEnd::Link` pops it, and if the link's text produced at least one span (`span_end > span_start`), stages `(span_start, span_end, target)` into `pending_links`. Staged links resolve to a concrete line index only at the next `flush_line` call (`src/markdown.rs:80-96`), since a link's owning `current`/`Line` may not be flushed until later in the same block (e.g. more text after the link, before the paragraph ends) — `pending_links` is drained into `links` at that flush, stamped with `lines.len()` (the index the flushed line is about to occupy). This assumes a link's start and end always fall within one `flush_line`-delimited segment (true today: nothing flushes mid-link since link contents are inline-only, no block-level breaks).

Images (`Tag::Image`) are styled but not tracked as links — not navigable targets for a file viewer.

## Known gap

Table rendering pushes bold styling and a two-space separator per cell (`src/markdown.rs:158-160`, `200-203`) but does not align columns or draw borders — cells just run together in reading order with no `Tag::Table` grid handling.
