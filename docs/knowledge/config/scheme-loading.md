---
type: concept
title: Color scheme config and loading
description: How ~/.term-markdown/config.toml + schemes/*.toml resolve into a render-ready Scheme.
resource: src/scheme.rs
tags: [config, toml, color-scheme, ratatui, syntect]
---

# Color scheme config and loading

`Scheme::load(config_path, cli_override) -> Result<Scheme>` (`src/scheme.rs:234`) is the sole
entry point, called once from `main` (`src/main.rs`) before `App` is constructed. The resulting
`Scheme` is stored on `App` for the process lifetime — no live-reload, no in-app switching (see
[app-loop](../tui/app-loop.md)). It's a pure filesystem + TOML-parsing module; no terminal I/O.

## Config file and scheme directory

`~/.term-markdown/config.toml` names the active scheme (`scheme = "name"`); scheme files live in
a **fixed sibling directory** next to the config file, `~/.term-markdown/schemes/<name>.toml`
(`scheme_path_for`, `src/scheme.rs:276-283`) — a direct `config_path.parent().join("schemes")`
join, not an ancestor walk like `bundle::detect_root` (see
[bundle-root](../tui/bundle-root.md)); the two are unrelated lookups that happen to share the
"find a file relative to another" shape. `default_config_path()` (`src/scheme.rs:265-269`)
resolves `~` by reading the `$HOME` env var directly, the same pattern (and same MSRV reasoning
— `std::env::home_dir` deprecated below 1.87, crate MSRV 1.85) as `bundle::walk_stop`; it
deliberately doesn't add a `dirs`/`directories` dependency, since this app only ever needs
`~/.term-markdown`, not XDG paths.

## Resolution precedence and error handling

Precedence: `--scheme NAME` CLI flag (`cli_override`) → `config.toml`'s `scheme` key → the
built-in default. `Scheme::load` (`src/scheme.rs:234-256`):

1. Reads `config.toml` via `read_config` (`src/scheme.rs:287-299`) — `Ok(None)` if the file
   doesn't exist (normal, not an error), `Err` if it exists but isn't valid TOML.
2. Picks `name` from `cli_override`, else the config's `scheme` key, else the literal string
   `"default"`.
3. If `config_path` is `None` (no `$HOME`) and no CLI override was given, falls back to
   `Scheme::default_builtin()` directly. If a CLI override *was* given with no `$HOME`
   resolvable, that's a hard error (a scheme dir has nowhere to be sought), not a silent
   fallback.
4. If `name == "default"` and `schemes/default.toml` doesn't exist on disk, uses
   `Scheme::default_builtin()` — the zero-config path: a fresh install with no
   `~/.term-markdown/` at all works immediately, with rendering identical to the original
   hardcoded colors.
5. Otherwise (`load_scheme_file`, `src/scheme.rs:301-307`) reads and parses the named scheme
   file and **hard-fails** (via `anyhow::Context`, naming the exact path/field) on: a missing
   named scheme file, malformed scheme TOML, an unparseable color string, or an unknown
   `syntect_theme` name. This mirrors `main.rs`'s existing convention of hard-failing on
   explicit-but-wrong input (e.g. `--root` validation) — an absent config/scheme is normal, but
   a named one that's broken is not silently ignored. Because `anyhow::Error`'s `Display` only
   shows the outermost context message, tests and callers that need the full chain (e.g. to
   confirm which field failed) must use the alternate format, `format!("{err:#}")`, not
   `.to_string()`.

## Scheme file shape

A scheme TOML has two tables, `[markdown]` (`MarkdownColors`, `src/scheme.rs:96-113`) and `[ui]`
(`UiColors`, `src/scheme.rs:116-122`) — one field per element listed in
[markdown-pipeline](../rendering/markdown-pipeline.md) and [app-loop](../tui/app-loop.md)
respectively. No palette/named-color indirection layer: TOML's own table nesting is judged
sufficient grouping at this scale (~20 fields total), and repeating a hex/named string in two
fields that happen to share a color is an acceptable authoring cost for schemes this small.

Every color-bearing field is a `StyleSpec` (`src/scheme.rs:28-35`) — `{ fg, bg, bold, italic,
underline, reversed }`, all sub-fields optional (`#[serde(default)]`). `fg`/`bg` strings are
parsed via `ratatui::style::Color`'s own `FromStr` (`parse_color`, `src/scheme.rs:65-69`), which
already supports both named colors (`"yellow"`, `"darkgray"`, ...) and `"#rrggbb"` hex — no
custom color parser needed. Two fields deliberately break the `StyleSpec`-everywhere pattern
because they only ever need a single value: `code_block_bg` is a bare color string (only ever
used as a background), and `syntect_theme` is a bare theme-name string (see below).

`[ui]`'s `background`/`border`/`title` fields additionally accept the literal string `"none"`
(`UiField`, `src/scheme.rs:75-93`, an untagged enum of `Sentinel(String) | Styled(StyleSpec)`)
meaning "inherit the terminal's/ratatui's default rendering" — resolved to `Option<Style>` where
`None` means exactly that. `status_bar` and `selection` have no such sentinel; they're always a
`StyleSpec`.

## Syntect theme selection

`syntect_theme` (a `[markdown]` field) names one of
`syntect::highlighting::ThemeSet::load_defaults`'s bundled themes (`base16-ocean.dark`,
`base16-eighties.dark`, `base16-mocha.dark`, `base16-ocean.light`, `InspiredGitHub`, `Solarized
(dark)`, `Solarized (light)`). It's resolved to an owned `Theme` once in `resolve()`
(`src/scheme.rs:309-360`, theme lookup at `src/scheme.rs:315-323`) and stored as
`Scheme.syntax_theme`, not re-loaded per `render()` call — `markdown::render` and
`highlight_code_block` just borrow `&scheme.syntax_theme` (see
[markdown-pipeline](../rendering/markdown-pipeline.md)). An unknown theme name is a hard error
at load time, not a lazy failure inside the render loop.

## Built-in default and the example asset

`Scheme::default_builtin()` (`src/scheme.rs:185-232`) is a hardcoded Rust literal reproducing
term-markdown's original colors exactly (yellow/cyan/magenta headings, `base16-ocean.dark`
syntect theme, etc.) — used whenever no config/scheme file resolves (see Resolution precedence
above). `assets/schemes/default.toml` is a human-readable, doc-only copy of the same values,
meant to be copied to `~/.term-markdown/schemes/<name>.toml` as a starting point — the running
code never reads this file. The two are kept in sync by hand; a test,
`scheme::tests::default_scheme_asset_matches_builtin` (`src/scheme.rs`), parses the asset file
and asserts every field matches `default_builtin()`'s, so drift between them fails `cargo test`
rather than being discovered as a rendering bug.

## CLI flag

`main.rs`'s `Args` (`src/main.rs`) has `--scheme NAME`, threaded into `Scheme::load` as
`cli_override`. It exists alongside the config file (not instead of it) as a one-field addition
to the same `clap`-derived struct that already has `--root`, useful for trying a scheme without
editing `config.toml`.
