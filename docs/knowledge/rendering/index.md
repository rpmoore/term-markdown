---
type: index
title: Rendering
description: Markdown-to-styled-Text rendering area index.
resource: src/markdown.rs
tags: [index, rendering]
---

# Rendering

Converts markdown source into a styled `ratatui::text::Text` for display. All logic lives in `src/markdown.rs`, called once per file load from `App::new` (`src/main.rs:41`).

## Concept docs

- [markdown-pipeline](markdown-pipeline.md) — event-driven parse/style pipeline, code-block syntax highlighting
