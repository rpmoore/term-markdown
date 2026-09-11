---
type: index
title: term-markdown knowledge bundle
description: Root index for term-markdown's OKF-conformant knowledge bundle.
resource: .
tags: [index]
---

# term-markdown Knowledge Bundle

Documents *current, implemented* behavior, grounded with `file:line` references. For design history (why a decision was made, what was rejected), see `docs/plans/` once that exists.

## Areas

- [rendering](rendering/index.md) — markdown → styled `ratatui::Text` (`src/markdown.rs`)
- [tui](tui/index.md) — app loop, terminal setup, scroll/event handling (`src/main.rs`)

No concept docs exist yet — add one under the relevant area the first time a change touches a subsystem with real invariants (see `AGENTS.md` § Knowledge Bundle for when a doc is warranted).
