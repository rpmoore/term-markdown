//! Color-scheme config and TOML loading.
//!
//! `~/.term-markdown/config.toml` names the active scheme; scheme files live
//! in a fixed `schemes/` directory next to the config file
//! (`~/.term-markdown/schemes/<name>.toml`) — a fixed sibling relationship,
//! not an ancestor walk like `bundle::detect_root`. An absent config file,
//! or a `scheme = "default"` with no matching file on disk, falls back to
//! [`Scheme::default_builtin`] (this crate's original hardcoded colors), so
//! the app works with zero configuration. A config or scheme file that
//! exists but is invalid, or a named scheme file that's missing, is a hard
//! error naming the offending path. Pure filesystem + parsing — no terminal
//! I/O.

use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use syntect::highlighting::{Theme, ThemeSet};

const DEFAULT_SCHEME_NAME: &str = "default";

/// A single styled element, deserialized from a scheme TOML inline table
/// like `{ fg = "yellow", bold = true }`. Every sub-field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StyleSpec {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
    reversed: bool,
}

impl StyleSpec {
    fn resolve(&self) -> Result<Style> {
        let mut style = Style::default();
        if let Some(fg) = &self.fg {
            style = style.fg(parse_color(fg)?);
        }
        if let Some(bg) = &self.bg {
            style = style.bg(parse_color(bg)?);
        }
        let mut modifier = Modifier::empty();
        if self.bold {
            modifier |= Modifier::BOLD;
        }
        if self.italic {
            modifier |= Modifier::ITALIC;
        }
        if self.underline {
            modifier |= Modifier::UNDERLINED;
        }
        if self.reversed {
            modifier |= Modifier::REVERSED;
        }
        Ok(style.add_modifier(modifier))
    }
}

/// Parses a hex (`"#rrggbb"`) or named (`"yellow"`, `"darkgray"`, ...) color
/// string via `ratatui::style::Color`'s own `FromStr`.
fn parse_color(s: &str) -> Result<Color> {
    Color::from_str(s).with_context(|| {
        format!("invalid color {s:?} (expected a named color like \"yellow\" or a hex value like \"#ffcc00\")")
    })
}

/// A UI-chrome field: either the literal `"none"` (inherit the terminal's
/// default rendering) or a style table.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum UiField {
    Sentinel(String),
    Styled(StyleSpec),
}

impl UiField {
    fn resolve(&self, field: &str) -> Result<Option<Style>> {
        match self {
            UiField::Sentinel(s) if s == "none" => Ok(None),
            UiField::Sentinel(s) => {
                bail!("scheme field `ui.{field}`: expected \"none\" or a style table, got {s:?}")
            }
            UiField::Styled(spec) => Ok(Some(
                spec.resolve()
                    .with_context(|| format!("scheme field `ui.{field}`"))?,
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkdownColors {
    heading_h1: StyleSpec,
    heading_h2: StyleSpec,
    heading_h3: StyleSpec,
    blockquote: StyleSpec,
    blockquote_marker: String,
    code_fence_marker: StyleSpec,
    code_block_bg: String,
    code_inline: StyleSpec,
    syntect_theme: String,
    list_marker: StyleSpec,
    link: StyleSpec,
    image_alt: StyleSpec,
    table_header_bold: bool,
    horizontal_rule: StyleSpec,
    horizontal_rule_glyph: String,
    horizontal_rule_width: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiColors {
    background: UiField,
    border: UiField,
    title: UiField,
    status_bar: StyleSpec,
    selection: StyleSpec,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemeFile {
    /// Informational only; not read back anywhere.
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
    markdown: MarkdownColors,
    ui: UiColors,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    scheme: Option<String>,
}

/// Resolved, render-ready markdown-element styles — the only form
/// `markdown::render` reads.
#[derive(Debug, Clone)]
pub struct MarkdownStyles {
    pub heading_h1: Style,
    pub heading_h2: Style,
    pub heading_h3: Style,
    pub blockquote: Style,
    pub blockquote_marker: String,
    pub code_fence_marker: Style,
    pub code_block_bg: Style,
    pub code_inline: Style,
    pub list_marker: Style,
    pub link: Style,
    pub image_alt: Style,
    pub table_header_bold: bool,
    pub horizontal_rule: Style,
    pub horizontal_rule_glyph: String,
    pub horizontal_rule_width: usize,
}

/// Resolved, render-ready UI-chrome styles. `None` means "inherit the
/// terminal's default rendering" (the `"none"` sentinel in scheme files).
#[derive(Debug, Clone)]
pub struct UiStyles {
    pub background: Option<Style>,
    pub border: Option<Style>,
    pub title: Option<Style>,
    pub status_bar: Style,
    pub selection: Style,
}

/// A fully resolved color scheme: every field is render-ready (`Style`s
/// already built, the syntect theme already looked up), so `markdown::render`
/// and `main.rs`'s draw code never touch raw TOML or color strings.
#[derive(Debug, Clone)]
pub struct Scheme {
    pub markdown: MarkdownStyles,
    pub ui: UiStyles,
    pub syntax_theme: Theme,
}

impl Scheme {
    /// The scheme used when no config/scheme file resolves. Must reproduce
    /// term-markdown's original hardcoded colors exactly; kept in sync by
    /// hand with `assets/schemes/default.toml` (checked by a test below).
    pub fn default_builtin() -> Scheme {
        let syntax_theme = ThemeSet::load_defaults().themes["base16-ocean.dark"].clone();
        Scheme {
            markdown: MarkdownStyles {
                heading_h1: Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
                blockquote: Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
                blockquote_marker: "\u{2503} ".to_string(),
                code_fence_marker: Style::default().fg(Color::DarkGray),
                code_block_bg: Style::default().bg(Color::Rgb(30, 30, 30)),
                code_inline: Style::default().fg(Color::Green).bg(Color::Rgb(40, 40, 40)),
                list_marker: Style::default().fg(Color::White),
                link: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
                image_alt: Style::default().fg(Color::Magenta),
                table_header_bold: true,
                horizontal_rule: Style::default().fg(Color::DarkGray),
                horizontal_rule_glyph: "\u{2500}".to_string(),
                horizontal_rule_width: 60,
            },
            ui: UiStyles {
                background: None,
                border: None,
                title: None,
                status_bar: Style::default().fg(Color::DarkGray),
                selection: Style::default().add_modifier(Modifier::REVERSED),
            },
            syntax_theme,
        }
    }

    /// Resolve the active scheme: `cli_override` (from `--scheme`) wins over
    /// `config_path`'s `scheme` key, which wins over the built-in default.
    ///
    /// The effective scheme name always resolves to `"default"` when there's
    /// no config file and no CLI override — but that alone doesn't guarantee
    /// [`Scheme::default_builtin`] is used: if `schemes/default.toml` exists
    /// next to where `config_path` *would* be (even with no `config.toml`
    /// there at all), it's loaded instead. This is deliberate — it lets a
    /// user reskin the default look by dropping a single file, with no
    /// `config.toml` boilerplate required — not an accidental fallback.
    /// `Scheme::default_builtin` is used only when no such file is present
    /// (the true zero-config case) or `$HOME` can't be resolved at all. A
    /// config/scheme file that exists but is invalid, or a named
    /// (non-`"default"`) scheme file that's missing, is a hard error naming
    /// the offending path.
    pub fn load(config_path: Option<&Path>, cli_override: Option<&str>) -> Result<Scheme> {
        let config = match config_path {
            Some(path) => read_config(path)?,
            None => None,
        };

        let name = cli_override
            .map(str::to_string)
            .or_else(|| config.and_then(|c| c.scheme))
            .unwrap_or_else(|| DEFAULT_SCHEME_NAME.to_string());
        validate_scheme_name(&name)?;

        let Some(config_path) = config_path else {
            if name != DEFAULT_SCHEME_NAME {
                bail!("cannot resolve scheme \"{name}\": no home directory found (is $HOME set?)");
            }
            return Ok(Scheme::default_builtin());
        };

        let scheme_path = scheme_path_for(config_path, &name);
        if name == DEFAULT_SCHEME_NAME && !default_scheme_file_exists(&scheme_path)? {
            return Ok(Scheme::default_builtin());
        }

        load_scheme_file(&scheme_path)
    }
}

/// Whether `path` exists as a regular file, for the `"default"`-scheme
/// existence check. Unlike `Path::is_file()`, which silently reports `false`
/// for any metadata failure (permission denied, a broken symlink) or when
/// the path is a directory, this treats anything other than "doesn't exist"
/// as a hard error — an existing-but-broken `schemes/default.toml` must not
/// be silently ignored in favor of the built-in default.
fn default_scheme_file_exists(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => bail!(
            "expected a file at {} but found something else (e.g. a directory)",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("failed to check scheme file {}", path.display())),
    }
}

/// The path `~/.term-markdown/config.toml`, or `None` if `$HOME` isn't set.
/// Reads `$HOME` directly rather than adding a `dirs`/`directories`
/// dependency — same pattern (and same MSRV reasoning) as
/// `bundle::walk_stop`.
pub fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".term-markdown")
            .join("config.toml"),
    )
}

/// Rejects a scheme name that would escape the `schemes/` directory once
/// joined with `.toml` (e.g. `../../etc/passwd`, an absolute path, or a
/// Windows drive-relative name like `C:evil`) — `scheme_path_for` does a
/// plain path join with no clamping, so this must run first on any
/// config/CLI-supplied name. Checks both that `name` parses as exactly one
/// `Component::Normal` (rejects `.`/`..`/root/prefix components — the
/// portable way to catch platform-specific quirks like Windows drive
/// prefixes) and that it contains no literal `/`/`\` (belt-and-suspenders:
/// `\` isn't a separator in a Unix build's `Component` parsing, so it
/// wouldn't be caught by the first check alone when compiled there).
fn validate_scheme_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    let is_single_normal_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let is_plain_segment =
        is_single_normal_component && !name.contains('/') && !name.contains('\\');
    if !is_plain_segment {
        bail!(
            "invalid scheme name {name:?}: must be a plain name with no path separators or \"..\""
        );
    }
    Ok(())
}

/// The scheme file for `name`, in the fixed `schemes/` directory next to
/// `config_path` — a sibling lookup, not an ancestor walk.
fn scheme_path_for(config_path: &Path, name: &str) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("schemes")
        .join(format!("{name}.toml"))
}

/// Reads and parses `path` as a config file. `Ok(None)` means the file
/// doesn't exist — not an error, since an absent config is the normal
/// zero-config state; `Err` means it exists but isn't valid TOML.
fn read_config(path: &Path) -> Result<Option<ConfigFile>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("failed to read config file {}", path.display()));
        }
    };
    let file: ConfigFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    Ok(Some(file))
}

fn load_scheme_file(path: &Path) -> Result<Scheme> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read scheme file {}", path.display()))?;
    let file: SchemeFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse scheme file {}", path.display()))?;
    resolve(file).with_context(|| format!("invalid scheme file {}", path.display()))
}

fn resolve(file: SchemeFile) -> Result<Scheme> {
    let SchemeFile {
        markdown: m, ui: u, ..
    } = file;

    let syntax_theme = ThemeSet::load_defaults()
        .themes
        .get(&m.syntect_theme)
        .cloned()
        .with_context(|| {
            format!(
                "unknown syntect_theme {:?} (markdown.syntect_theme)",
                m.syntect_theme
            )
        })?;

    const MAX_RULE_WIDTH: usize = 1000;
    if m.horizontal_rule_glyph.is_empty() {
        bail!("markdown.horizontal_rule_glyph: must not be empty");
    }
    if m.horizontal_rule_width == 0 || m.horizontal_rule_width > MAX_RULE_WIDTH {
        bail!(
            "markdown.horizontal_rule_width: must be between 1 and {MAX_RULE_WIDTH} (got {})",
            m.horizontal_rule_width
        );
    }

    let markdown = MarkdownStyles {
        heading_h1: m.heading_h1.resolve().context("markdown.heading_h1")?,
        heading_h2: m.heading_h2.resolve().context("markdown.heading_h2")?,
        heading_h3: m.heading_h3.resolve().context("markdown.heading_h3")?,
        blockquote: m.blockquote.resolve().context("markdown.blockquote")?,
        blockquote_marker: m.blockquote_marker,
        code_fence_marker: m
            .code_fence_marker
            .resolve()
            .context("markdown.code_fence_marker")?,
        code_block_bg: Style::default()
            .bg(parse_color(&m.code_block_bg).context("markdown.code_block_bg")?),
        code_inline: m.code_inline.resolve().context("markdown.code_inline")?,
        list_marker: m.list_marker.resolve().context("markdown.list_marker")?,
        link: m.link.resolve().context("markdown.link")?,
        image_alt: m.image_alt.resolve().context("markdown.image_alt")?,
        table_header_bold: m.table_header_bold,
        horizontal_rule: m
            .horizontal_rule
            .resolve()
            .context("markdown.horizontal_rule")?,
        horizontal_rule_glyph: m.horizontal_rule_glyph,
        horizontal_rule_width: m.horizontal_rule_width,
    };

    let ui = UiStyles {
        background: u.background.resolve("background")?,
        border: u.border.resolve("border")?,
        title: u.title.resolve("title")?,
        status_bar: u.status_bar.resolve().context("ui.status_bar")?,
        selection: u.selection.resolve().context("ui.selection")?,
    };

    Ok(Scheme {
        markdown,
        ui,
        syntax_theme,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("term-markdown-scheme-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    const VALID_SCHEME: &str = r##"
        name = "custom"

        [markdown]
        heading_h1        = { fg = "red", bold = true }
        heading_h2        = { fg = "cyan" }
        heading_h3        = { fg = "magenta" }
        blockquote        = { fg = "darkgray", italic = true }
        blockquote_marker = "> "
        code_fence_marker = { fg = "darkgray" }
        code_block_bg     = "#101010"
        code_inline       = { fg = "green", bg = "#202020" }
        syntect_theme     = "Solarized (dark)"
        list_marker       = { fg = "white" }
        link              = { fg = "blue", underline = true }
        image_alt         = { fg = "magenta" }
        table_header_bold = true
        horizontal_rule       = { fg = "darkgray" }
        horizontal_rule_glyph = "-"
        horizontal_rule_width = 40

        [ui]
        background = "none"
        border     = "none"
        title      = "none"
        status_bar = { fg = "darkgray" }
        selection  = { reversed = true }
    "##;

    #[test]
    fn load_falls_back_to_builtin_when_config_path_is_none() {
        let scheme = Scheme::load(None, None).unwrap();
        assert_eq!(
            scheme.markdown.heading_h1,
            Scheme::default_builtin().markdown.heading_h1
        );
    }

    #[test]
    fn load_falls_back_to_builtin_when_config_file_absent() {
        let t = temp_tree("absent-config");
        let config_path = t.join("config.toml");

        let scheme = Scheme::load(Some(&config_path), None).unwrap();
        assert_eq!(
            scheme.markdown.heading_h1,
            Scheme::default_builtin().markdown.heading_h1
        );

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn default_scheme_path_that_is_a_directory_is_a_hard_error() {
        // An existing-but-broken schemes/default.toml (here: a directory
        // instead of a file) must not be silently swallowed into the
        // built-in-default fallback the way a genuinely absent file is.
        let t = temp_tree("default-scheme-is-dir");
        let config_path = t.join("config.toml");
        std::fs::create_dir_all(t.join("schemes/default.toml")).unwrap();

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(err.to_string().contains("default.toml"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn load_reads_named_scheme_from_sibling_schemes_dir() {
        let t = temp_tree("named-scheme");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"custom\"\n");
        touch(&t.join("schemes/custom.toml"), VALID_SCHEME);

        let scheme = Scheme::load(Some(&config_path), None).unwrap();
        assert_eq!(scheme.markdown.heading_h1.fg, Some(Color::Red));
        assert_eq!(scheme.markdown.horizontal_rule_width, 40);

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn cli_override_takes_precedence_over_config_file() {
        let t = temp_tree("cli-precedence");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"from-config\"\n");
        touch(&t.join("schemes/from-cli.toml"), VALID_SCHEME);
        // Deliberately no "from-config.toml" — if the CLI override didn't
        // win, this would fail to load instead of succeeding.

        let scheme = Scheme::load(Some(&config_path), Some("from-cli")).unwrap();
        assert_eq!(scheme.markdown.heading_h1.fg, Some(Color::Red));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn malformed_config_toml_is_a_hard_error() {
        let t = temp_tree("bad-config");
        let config_path = t.join("config.toml");
        touch(&config_path, "this is not valid toml {{{");

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(err.to_string().contains("config file"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn missing_named_scheme_file_is_a_hard_error() {
        let t = temp_tree("missing-scheme");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"nope\"\n");

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(err.to_string().contains("nope.toml"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn malformed_scheme_toml_is_a_hard_error() {
        let t = temp_tree("bad-scheme");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"broken\"\n");
        touch(&t.join("schemes/broken.toml"), "not valid toml {{{");

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(err.to_string().contains("broken.toml"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn bad_color_string_is_a_hard_error() {
        let t = temp_tree("bad-color");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"bad-color\"\n");
        let bad = VALID_SCHEME.replace("fg = \"red\"", "fg = \"not-a-color\"");
        touch(&t.join("schemes/bad-color.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("heading_h1"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn unknown_syntect_theme_is_a_hard_error() {
        let t = temp_tree("bad-theme");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"bad-theme\"\n");
        let bad = VALID_SCHEME.replace("Solarized (dark)", "not-a-real-theme");
        touch(&t.join("schemes/bad-theme.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("syntect_theme"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn cli_override_without_home_is_a_hard_error() {
        let err = Scheme::load(None, Some("custom")).unwrap_err();
        assert!(err.to_string().contains("custom"));
    }

    #[test]
    fn cli_override_default_without_home_falls_back_to_builtin() {
        // "--scheme default" is a no-op equivalent to omitting the flag, so
        // it must not require a resolvable $HOME the way a real named
        // scheme does.
        let scheme = Scheme::load(None, Some("default")).unwrap();
        assert_eq!(
            scheme.markdown.heading_h1,
            Scheme::default_builtin().markdown.heading_h1
        );
    }

    #[test]
    fn scheme_name_with_path_traversal_is_rejected() {
        let t = temp_tree("path-traversal");
        let config_path = t.join("dir/config.toml");
        touch(&config_path, "scheme = \"../evil\"\n");
        // Deliberately place a file at the traversal target (schemes/../evil.toml,
        // i.e. sibling of "schemes") to prove it would otherwise have been read.
        touch(&t.join("dir/evil.toml"), VALID_SCHEME);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(err.to_string().contains("invalid scheme name"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn cli_scheme_name_with_slash_is_rejected() {
        let err = Scheme::load(None, Some("sub/dir")).unwrap_err();
        assert!(err.to_string().contains("invalid scheme name"));
    }

    #[test]
    fn validate_scheme_name_accepts_plain_names() {
        assert!(validate_scheme_name("default").is_ok());
        assert!(validate_scheme_name("my-scheme_1").is_ok());
    }

    #[test]
    fn validate_scheme_name_rejects_dot_and_dotdot() {
        assert!(validate_scheme_name(".").is_err());
        assert!(validate_scheme_name("..").is_err());
    }

    #[test]
    fn validate_scheme_name_rejects_backslash() {
        // Not a path separator under a Unix build's `Component` parsing, so
        // this must be caught by the explicit character check, not just
        // `Path::components()`.
        assert!(validate_scheme_name("a\\b").is_err());
    }

    #[test]
    fn validate_scheme_name_rejects_root_and_empty() {
        assert!(validate_scheme_name("/").is_err());
        assert!(validate_scheme_name("").is_err());
    }

    #[test]
    fn zero_horizontal_rule_width_is_a_hard_error() {
        let t = temp_tree("rule-zero-width");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"bad-rule\"\n");
        let bad = VALID_SCHEME.replace("horizontal_rule_width = 40", "horizontal_rule_width = 0");
        touch(&t.join("schemes/bad-rule.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("horizontal_rule_width"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn oversized_horizontal_rule_width_is_a_hard_error() {
        let t = temp_tree("rule-huge-width");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"bad-rule\"\n");
        let bad = VALID_SCHEME.replace(
            "horizontal_rule_width = 40",
            "horizontal_rule_width = 999999",
        );
        touch(&t.join("schemes/bad-rule.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("horizontal_rule_width"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn empty_horizontal_rule_glyph_is_a_hard_error() {
        let t = temp_tree("rule-empty-glyph");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"bad-rule\"\n");
        let bad = VALID_SCHEME.replace(
            "horizontal_rule_glyph = \"-\"",
            "horizontal_rule_glyph = \"\"",
        );
        touch(&t.join("schemes/bad-rule.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("horizontal_rule_glyph"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn unknown_field_in_markdown_table_is_a_hard_error() {
        let t = temp_tree("unknown-markdown-field");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"typo\"\n");
        // "boldd" instead of "bold" inside heading_h1's inline table.
        let bad = VALID_SCHEME.replace(
            "heading_h1        = { fg = \"red\", bold = true }",
            "heading_h1        = { fg = \"red\", boldd = true }",
        );
        touch(&t.join("schemes/typo.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("boldd"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn unknown_top_level_field_in_scheme_file_is_a_hard_error() {
        let t = temp_tree("unknown-toplevel-field");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheme = \"typo\"\n");
        let bad = format!("extra_section = true\n{VALID_SCHEME}");
        touch(&t.join("schemes/typo.toml"), &bad);

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("extra_section"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn unknown_key_in_config_toml_is_a_hard_error() {
        let t = temp_tree("unknown-config-key");
        let config_path = t.join("config.toml");
        touch(&config_path, "scheem = \"typo\"\n");

        let err = Scheme::load(Some(&config_path), None).unwrap_err();
        assert!(format!("{err:#}").contains("scheem"));

        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn style_spec_resolves_hex_and_named_colors() {
        let hex = StyleSpec {
            fg: Some("#112233".to_string()),
            ..Default::default()
        };
        assert_eq!(
            hex.resolve().unwrap().fg,
            Some(Color::Rgb(0x11, 0x22, 0x33))
        );

        let named = StyleSpec {
            fg: Some("yellow".to_string()),
            ..Default::default()
        };
        assert_eq!(named.resolve().unwrap().fg, Some(Color::Yellow));
    }

    #[test]
    fn style_spec_folds_all_modifiers() {
        let spec = StyleSpec {
            bold: true,
            italic: true,
            underline: true,
            reversed: true,
            ..Default::default()
        };
        let style = spec.resolve().unwrap();
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn style_spec_rejects_invalid_color() {
        let spec = StyleSpec {
            fg: Some("not-a-color".to_string()),
            ..Default::default()
        };
        assert!(spec.resolve().is_err());
    }

    #[test]
    fn default_scheme_asset_matches_builtin() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/schemes/default.toml"
        ))
        .unwrap();
        let file: SchemeFile = toml::from_str(&raw).unwrap();
        let from_asset = resolve(file).unwrap();
        let builtin = Scheme::default_builtin();

        assert_eq!(from_asset.markdown.heading_h1, builtin.markdown.heading_h1);
        assert_eq!(from_asset.markdown.heading_h2, builtin.markdown.heading_h2);
        assert_eq!(from_asset.markdown.heading_h3, builtin.markdown.heading_h3);
        assert_eq!(from_asset.markdown.blockquote, builtin.markdown.blockquote);
        assert_eq!(
            from_asset.markdown.blockquote_marker,
            builtin.markdown.blockquote_marker
        );
        assert_eq!(
            from_asset.markdown.code_fence_marker,
            builtin.markdown.code_fence_marker
        );
        assert_eq!(
            from_asset.markdown.code_block_bg,
            builtin.markdown.code_block_bg
        );
        assert_eq!(
            from_asset.markdown.code_inline,
            builtin.markdown.code_inline
        );
        assert_eq!(
            from_asset.markdown.list_marker,
            builtin.markdown.list_marker
        );
        assert_eq!(from_asset.markdown.link, builtin.markdown.link);
        assert_eq!(from_asset.markdown.image_alt, builtin.markdown.image_alt);
        assert_eq!(
            from_asset.markdown.table_header_bold,
            builtin.markdown.table_header_bold
        );
        assert_eq!(
            from_asset.markdown.horizontal_rule,
            builtin.markdown.horizontal_rule
        );
        assert_eq!(
            from_asset.markdown.horizontal_rule_glyph,
            builtin.markdown.horizontal_rule_glyph
        );
        assert_eq!(
            from_asset.markdown.horizontal_rule_width,
            builtin.markdown.horizontal_rule_width
        );
        assert_eq!(from_asset.ui.background, builtin.ui.background);
        assert_eq!(from_asset.ui.border, builtin.ui.border);
        assert_eq!(from_asset.ui.title, builtin.ui.title);
        assert_eq!(from_asset.ui.status_bar, builtin.ui.status_bar);
        assert_eq!(from_asset.ui.selection, builtin.ui.selection);
        assert_eq!(from_asset.syntax_theme, builtin.syntax_theme);
    }
}
