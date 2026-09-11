---
type: concept
title: Bundle root detection
description: How the viewer decides which directory absolute (/x/y.md) links are relative to.
resource: src/bundle.rs
tags: [tui, okf, links]
---

# Bundle root detection

## Why it exists

[OKF §5.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md) defines
absolute links (`/x/y.md`) as relative to the **bundle root** — and recommends them over relative
links because they survive moving a document within the bundle. But nothing machine-readable
declares where a bundle's root is: the root `index.md` *may* carry `okf_version` frontmatter, most
subdirectories carry an `index.md` of their own, and the chain of `index.md`s can have gaps. So the
viewer has to guess, and let the user override the guess. `src/bundle.rs` is that guess; the
override is `--root <dir>`. Detection runs once in `main` and the result is stored on
`App.bundle_root` for the life of the session (see [app-loop](app-loop.md)).

## Algorithm

`detect_root(file)` (`src/bundle.rs:18-23`):

1. Canonicalize `file` — falling back to `std::path::absolute` if it doesn't exist — so a relative
   CLI path (`cargo run -- docs/knowledge/index.md`) has real ancestors and a symlinked entry point
   still finds the bundle it points into. This canonical path is used for *detection only*;
   `App.path` keeps the path the user typed.
2. Start from the file's directory and compute a `stop` via `walk_stop` (`src/bundle.rs:30-36`): the
   first ancestor the walk must **not** inspect.
   - Inside a git checkout (nearest ancestor with a `.git` entry — a directory, or a file for
     worktrees/submodules; `git_toplevel`, `src/bundle.rs:67-72`, no shell-out): the *parent* of
     the toplevel, so the toplevel itself is still a candidate.
   - Else, if the file is strictly under `$HOME`: `$HOME` itself, so a stray `~/index.md` can't
     become the root of every file under the home directory.
   - Else: none — walk all the way to `/`.
3. `detect_root_bounded(start, stop)` (`src/bundle.rs:47-63`) walks `start.ancestors()`, breaking at
   `stop`, and returns:
   1. the **nearest** directory whose `index.md` declares `okf_version` in its frontmatter
      (`declares_okf_version`, `src/bundle.rs:79-97`) — the spec permits frontmatter only in the
      bundle-root `index.md`, so this is definitive and the walk stops immediately;
   2. otherwise the **outermost** directory (before `stop`) containing an `index.md` *file* — gaps
      are fine, a `plans/` without an `index.md` between two directories that have one doesn't end
      the walk;
   3. otherwise `start` itself, which reduces to the pre-bundle-aware behavior (absolute links
      resolve against the file's own directory).

`declares_okf_version` is deliberately local rather than reusing `markdown::strip_frontmatter`: the
first line must be `---` (CRLF-tolerant — looser than `strip_frontmatter`, which affects only
detection), it scans to the closing `---` (the file is already in memory, so a long `tags:` list
can't hide the key), an unclosed fence counts as "no frontmatter" (matching `strip_frontmatter`),
and a key that only appears in the body after the closing fence doesn't count.

## Worked examples

- term-markdown's own `docs/knowledge/`: `index.md` declares `okf_version: "0.2"`, so rule 1 fires
  from any file under it — and does so even in a tarball checkout with no `.git`.
- A bundle whose root `index.md` has no frontmatter, inside a git repo, with nested `index.md`s and
  a gap (`docs/plans/` has none, `docs/plans/x/sections/` does): rule 2 picks `docs/` from any file,
  because the walk is bounded at the repo toplevel and `docs/` is the outermost `index.md` below it.

## Known misdetections (all fixed with `--root`)

- A nested repo or submodule *between* the file and the bundle root: the walk stops at the inner
  `.git` and falls to rule 3.
- A monorepo with an undeclared top-level `index.md` (a docs-site landing page) *and* an undeclared
  `docs/knowledge/index.md`: rule 2 picks the repo root. Declaring `okf_version` in the real root
  fixes it without `--root`.
- A `stop` that isn't actually an ancestor of `start` (can't happen via `walk_stop`, but
  `detect_root_bounded` doesn't check): the walk simply reaches `/`.
- Reported roots are canonical (`/private/tmp/...` rather than `/tmp/...` on macOS), which shows
  up in `(bundle root: …)` status messages.

## Known gaps

- A `:LINE` (or `:LINE:COL`) suffix is used only to find the file; the viewer doesn't scroll to
  that line.
- A non-markdown target (`deploy.go`) reached via the literal-path fallback is rendered as markdown.
  This is pre-existing behavior — relative links to source files do the same.
