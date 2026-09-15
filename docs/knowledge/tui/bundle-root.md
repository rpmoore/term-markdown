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
2. Start from the file's directory and compute a `stop` via `walk_stop` (`src/bundle.rs:30-38`): the
   first ancestor the walk must **not** inspect.
   - Inside a git checkout (nearest ancestor with a `.git` entry — a directory, or a file for
     worktrees/submodules; `git_toplevel`, `src/bundle.rs:69-74`, no shell-out): the *parent* of
     the toplevel, so the toplevel itself is still a candidate.
   - Else, if the file is strictly under `$HOME` (read from the environment): `$HOME` itself, so a
     stray `~/index.md` can't become the root of every file under the home directory.
   - Else: none — walk all the way to `/`.
3. `detect_root_bounded(start, stop)` (`src/bundle.rs:49-65`) walks `start.ancestors()`, breaking at
   `stop`, and returns:
   1. the **nearest** directory whose `index.md` declares a *top-level* `okf_version` key in its
      frontmatter (`declares_okf_version`, `src/bundle.rs:114-153`) — the spec permits frontmatter
      only in the bundle-root `index.md`, so this is definitive and the walk stops immediately;
   2. otherwise the **outermost** directory (before `stop`) containing an `index.md` *file* — gaps
      are fine, a `plans/` without an `index.md` between two directories that have one doesn't end
      the walk;
   3. otherwise `start` itself, which reduces to the pre-bundle-aware behavior (absolute links
      resolve against the file's own directory).

`declares_okf_version` is deliberately local rather than reusing `markdown::strip_frontmatter`: the
first line must be `---` (CRLF-tolerant — looser than `strip_frontmatter`, which affects only
detection), an unclosed fence counts as "no frontmatter" (matching `strip_frontmatter`), a key that
only appears in the body after the closing fence doesn't count, and neither does an indented
`okf_version:` nested under another key or inside a block scalar (column 0 only). It reads via a
`BufReader` over a `Read::take`-limited file handle rather than `fs::read_to_string`, and bails once
either the closing fence is found or `MAX_FRONTMATTER_SCAN_BYTES` (64 KiB, `src/bundle.rs:86`) bytes
have been consumed without one — this walk runs on every file open with no user interaction gating
it, so an `index.md` that is huge, or has a fence that never closes, or is a single pathological
line with no newline for megabytes, must not force a full (or even partial-but-unbounded) read into
memory. The limit is enforced via `Read::take` on the underlying file rather than counting bytes
returned by each `read_line` call after the fact: `read_line` itself has no length cap, so a single
huge unterminated line would otherwise be read into memory in one call regardless of a manual
post-hoc byte count. A side effect of reading line-by-line instead of the whole file up front: only
the frontmatter block itself needs to be valid UTF-8 now — invalid bytes later in the document body
(past the closing fence, or past the byte cap) are never read and so no longer affect the result,
unlike the old whole-file `fs::read_to_string`.

A line with no trailing newline only counts as a genuine closing `---` if it also reached real
end-of-file, not just the byte cap — `Take` cutting a line short mid-read looks identical to a
short final line at true EOF, so without this check a line that merely *starts* with `---` and
keeps going unclosed could be mistaken for a real closing fence if the cap happened to land exactly
three bytes in. This trades away one vanishingly unlikely case — a file whose real, valid closing
fence sits with no trailing newline at a byte offset exactly equal to the cap — in favor of never
producing a false match; the traded-away case still falls through to "no frontmatter", which is the
safe direction to be wrong in (fixable with `--root`).

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
