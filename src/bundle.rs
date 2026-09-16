//! OKF bundle-root detection.
//!
//! Absolute links (`/x/y.md`) in an OKF bundle are relative to the bundle
//! root, not the host filesystem, but nothing machine-readable declares
//! where that root is: the root `index.md` *may* carry `okf_version`
//! frontmatter, and most subdirectories have an `index.md` of their own.
//! These helpers implement the heuristic used when `--root` isn't given.
//! Pure filesystem lookups only — no terminal I/O.

use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// Bundle root for the bundle containing `file`, detected by walking up from
/// its directory. See [`detect_root_bounded`] for the rules; the walk stops
/// at [`walk_stop`]. Symlinks are resolved for detection only, so a
/// symlinked entry point still finds the real bundle.
pub fn detect_root(file: &Path) -> io::Result<PathBuf> {
    let file = fs::canonicalize(file).or_else(|_| std::path::absolute(file))?;
    let dir = file.parent().unwrap_or(&file);
    let stop = walk_stop(dir);
    Ok(detect_root_bounded(dir, stop.as_deref()))
}

/// The first ancestor of `start` that the root walk must *not* inspect:
/// the parent of the git toplevel (so the toplevel itself is still a
/// candidate), else `$HOME` when `start` is strictly inside it (a stray
/// `~/index.md` must not become the root of every file under the home
/// directory), else `None` — walk all the way to the filesystem root.
fn walk_stop(start: &Path) -> Option<PathBuf> {
    if let Some(toplevel) = git_toplevel(start) {
        return toplevel.parent().map(Path::to_path_buf);
    }
    // Read `$HOME` directly: `std::env::home_dir` is deprecated on
    // toolchains older than 1.87, and this crate's MSRV is 1.85.
    let home = PathBuf::from(std::env::var_os("HOME")?);
    (start != home && start.starts_with(&home)).then_some(home)
}

/// Walk `start` and its ancestors, breaking before `stop`, and pick the root:
///
/// 1. the first directory whose `index.md` declares `okf_version` in its
///    frontmatter — the spec permits frontmatter only in the bundle-root
///    `index.md`, so the nearest such directory is the enclosing bundle;
/// 2. otherwise the *outermost* directory containing an `index.md` file
///    (gaps in the chain are fine — a `plans/` without one between two
///    directories that have one doesn't break the walk);
/// 3. otherwise `start` itself.
fn detect_root_bounded(start: &Path, stop: Option<&Path>) -> PathBuf {
    let mut outermost: Option<&Path> = None;
    for dir in start.ancestors() {
        if Some(dir) == stop {
            break;
        }
        let index = dir.join("index.md");
        if !index.is_file() {
            continue;
        }
        if declares_okf_version(&index) {
            return dir.to_path_buf();
        }
        outermost = Some(dir);
    }
    outermost.unwrap_or(start).to_path_buf()
}

/// Nearest ancestor of `start` (inclusive) containing a `.git` entry —
/// a directory for a normal checkout, a file for worktrees and submodules.
fn git_toplevel(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Upper bound on bytes read looking for the closing frontmatter fence. A
/// directory being walked for bundle-root detection may contain an
/// `index.md` that is huge (or an unclosed `---` that never resolves) — this
/// bounds both the memory and time `declares_okf_version` can burn on a
/// single candidate, in a walk that runs on every file open with no user
/// interaction to gate it. Enforced by wrapping the file in [`Read::take`]
/// rather than counting bytes per line after the fact: a `read_line` call
/// has no length limit of its own, so a single pathological line with no
/// newline for tens of megabytes would otherwise be read into memory in one
/// call before a manual byte count ever got a chance to reject it.
const MAX_FRONTMATTER_SCAN_BYTES: u64 = 64 * 1024;

/// Whether `index` opens with a `---` frontmatter block that contains a
/// top-level `okf_version:` key. An unreadable file, no opening fence, an
/// unclosed fence (mirroring `markdown::strip_frontmatter`), or a fence that
/// doesn't close within [`MAX_FRONTMATTER_SCAN_BYTES`] all count as "no", as
/// does an indented `okf_version:` nested under another key or inside a
/// block scalar. Reads line-by-line via a `BufReader` over a
/// [`Read::take`]-limited handle rather than `fs::read_to_string`, so a huge
/// `index.md` is never loaded in full regardless of line length: a file that
/// doesn't even open with `---` costs one (bounded) line read, and a
/// pathological file with no closing fence is abandoned once the byte cap is
/// hit rather than read to EOF. Only the frontmatter block itself needs to
/// be valid UTF-8 — invalid bytes in the document body, past the closing
/// fence (or past the byte cap), are never read and so don't affect the
/// result, unlike the old whole-file `fs::read_to_string`.
///
/// A line with no trailing `\n` that also exhausted the byte budget is
/// treated as unclosed rather than checked against `"---"`: `Take` hitting
/// its limit mid-line is indistinguishable from genuine end-of-file by the
/// return value alone, so without this check a line that merely *starts*
/// with `---` (and continues for a long time after) could be mistaken for a
/// real closing fence if the cap happened to land exactly three bytes in.
/// The one false negative this trades away — a file whose real, valid
/// closing fence sits with no trailing newline at a byte offset exactly
/// equal to the cap — is left unresolved deliberately: erring toward "not a
/// bundle root" is safe (fixable with `--root`), where erring toward a false
/// match is not.
fn declares_okf_version(index: &Path) -> bool {
    let Ok(file) = fs::File::open(index) else {
        return false;
    };
    let mut reader = BufReader::new(file.take(MAX_FRONTMATTER_SCAN_BYTES));
    let mut line = String::new();
    let Ok(n) = reader.read_line(&mut line) else {
        return false;
    };
    let mut consumed = n as u64;
    if line.trim_end() != "---" {
        return false;
    }

    let mut found = false;
    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            // Genuine EOF, or invalid UTF-8: unclosed fence either way.
            Ok(0) | Err(_) => return false,
            Ok(n) => n,
        };
        consumed += n as u64;
        // A line with no trailing `\n` is ambiguous unless it also hit
        // genuine EOF: `Take` returning early once the scan cap is spent
        // looks identical to a real end-of-file mid-line, so a line
        // truncated right after an incidental "---" could otherwise be
        // mistaken for a real closing fence it never reached.
        if !line.ends_with('\n') && consumed >= MAX_FRONTMATTER_SCAN_BYTES {
            return false;
        }
        let trimmed = line.trim_end();
        if trimmed == "---" {
            return found;
        }
        if trimmed.starts_with("okf_version:") {
            found = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("term-markdown-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    const DECLARED: &str = "---\nokf_version: \"0.2\"\n---\n\n# Root\n";

    #[test]
    fn okf_version_index_is_root_even_when_outer_index_exists() {
        let t = temp_tree("bundle-declared");
        touch(&t.join("index.md"), "# outer\n");
        touch(&t.join("a/index.md"), DECLARED);
        touch(&t.join("a/b/index.md"), "# inner\n");
        let start = t.join("a/b/c");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(detect_root_bounded(&start, t.parent()), t.join("a"));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn outermost_index_before_stop_is_root_despite_gap() {
        let t = temp_tree("bundle-gap");
        touch(&t.join("index.md"), "# decoy above the repo\n");
        touch(&t.join("repo/docs/index.md"), "# docs root\n");
        touch(
            &t.join("repo/docs/plans/x/sections/index.md"),
            "# sections\n",
        );
        let start = t.join("repo/docs/plans/x/sections");

        assert_eq!(detect_root_bounded(&start, Some(&t)), t.join("repo/docs"));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn stop_dir_is_not_inspected() {
        let t = temp_tree("bundle-stop");
        touch(&t.join("index.md"), "# at stop\n");
        touch(&t.join("repo/index.md"), "# repo\n");
        let start = t.join("repo/a");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(detect_root_bounded(&start, Some(&t)), t.join("repo"));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn no_index_falls_back_to_start_dir() {
        let t = temp_tree("bundle-noindex");
        let start = t.join("a/b");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(detect_root_bounded(&start, t.parent()), start);
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn index_md_directory_is_ignored() {
        let t = temp_tree("bundle-indexdir");
        std::fs::create_dir_all(t.join("index.md")).unwrap();
        touch(&t.join("a/index.md"), "# a\n");
        let start = t.join("a/b");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(detect_root_bounded(&start, t.parent()), t.join("a"));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn git_toplevel_finds_dot_git_dir() {
        let t = temp_tree("bundle-gitdir");
        std::fs::create_dir_all(t.join("repo/.git")).unwrap();
        let start = t.join("repo/a/b");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(git_toplevel(&start), Some(t.join("repo")));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn git_toplevel_accepts_dot_git_file() {
        let t = temp_tree("bundle-gitfile");
        touch(&t.join("repo/.git"), "gitdir: ../.git/worktrees/repo\n");
        let start = t.join("repo/a");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(git_toplevel(&start), Some(t.join("repo")));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn walk_stop_prefers_git_toplevel_parent() {
        let t = temp_tree("bundle-walkstop");
        std::fs::create_dir_all(t.join("repo/.git")).unwrap();
        let start = t.join("repo/a");
        std::fs::create_dir_all(&start).unwrap();

        assert_eq!(walk_stop(&start), Some(t.clone()));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_true_when_key_in_frontmatter() {
        let t = temp_tree("bundle-fm-yes");
        let index = t.join("index.md");
        touch(&index, DECLARED);
        assert!(declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_without_key() {
        let t = temp_tree("bundle-fm-nokey");
        let index = t.join("index.md");
        touch(&index, "---\ntitle: x\n---\n# Body\n");
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_when_key_only_in_body() {
        let t = temp_tree("bundle-fm-body");
        let index = t.join("index.md");
        touch(&index, "---\ntitle: x\n---\n\nokf_version: \"0.2\"\n");
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_true_with_long_frontmatter() {
        let t = temp_tree("bundle-fm-long");
        let index = t.join("index.md");
        let tags: String = (0..200).map(|i| format!("  - tag{i}\n")).collect();
        touch(
            &index,
            &format!("---\nokf_version: \"0.2\"\ntags:\n{tags}---\n# Root\n"),
        );
        assert!(declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_when_key_is_nested() {
        let t = temp_tree("bundle-fm-nested");
        let index = t.join("index.md");
        touch(
            &index,
            "---\nmetadata:\n  okf_version: \"0.2\"\nnotes: |\n  okf_version: \"0.2\"\n---\n",
        );
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_when_fence_unclosed() {
        let t = temp_tree("bundle-fm-unclosed");
        let index = t.join("index.md");
        touch(&index, "---\nokf_version: \"0.2\"\n\n# never closed\n");
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_when_fence_never_closes_within_scan_cap() {
        // Regression test: a huge or hostile `index.md` with an opening
        // fence that never closes must be abandoned once the scan cap is
        // hit, not read to EOF (previously `fs::read_to_string` loaded the
        // whole file first, so a 1 GiB unclosed-fence file was fully
        // buffered in memory before this function even started scanning).
        let t = temp_tree("bundle-fm-scan-cap");
        let index = t.join("index.md");
        let filler: String = "tags:\n".repeat(MAX_FRONTMATTER_SCAN_BYTES as usize);
        touch(&index, &format!("---\nokf_version: \"0.2\"\n{filler}"));
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_for_single_huge_unterminated_line_within_cap() {
        // Regression test for the gap in the first version of the scan-cap
        // fix: `BufRead::read_line` has no length limit of its own, so a
        // single pathological line with no newline for many megabytes would
        // be read into memory in one call, before any manual byte-count
        // check ran. The fix wraps the file in `Read::take` so the total
        // bytes read is bounded regardless of line length; this file is one
        // line, several times over `MAX_FRONTMATTER_SCAN_BYTES`, with no
        // newline anywhere after the opening fence.
        let t = temp_tree("bundle-fm-huge-line");
        let index = t.join("index.md");
        let huge_line = "x".repeat(MAX_FRONTMATTER_SCAN_BYTES as usize * 4);
        touch(&index, &format!("---\n{huge_line}"));
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_when_cap_truncates_mid_line_onto_a_coincidental_dash_run() {
        // Regression test for a false positive in the `Read::take`-based
        // scan cap: a line that merely *starts* with "---" but keeps going
        // unclosed must not be mistaken for a real closing fence just
        // because the byte cap happens to land exactly three bytes into it.
        // The preceding filler is one complete, newline-terminated line so
        // the vulnerable line starts fresh at its own "-"; the cap is sized
        // to land exactly after that line's third byte, so a version of the
        // fix that trusted any line reading exactly "---" (rather than
        // checking it actually hit genuine EOF, not just the cap) would
        // wrongly return `true` here.
        let t = temp_tree("bundle-fm-cap-dash-coincidence");
        let index = t.join("index.md");
        let prefix = "---\nokf_version: \"0.2\"\n";
        let filler_len = MAX_FRONTMATTER_SCAN_BYTES as usize - 3 - prefix.len() - 1;
        let mut content = String::from(prefix);
        content.push_str(&"a".repeat(filler_len));
        content.push('\n'); // filler is a complete line, so the next line starts fresh
        content.push_str("---"); // cap lands exactly here, mid-line
        content.push_str(&"z".repeat(1000)); // proves the line never actually closes
        touch(&index, &content);
        assert!(!declares_okf_version(&index));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn declares_okf_version_false_without_frontmatter_or_file() {
        let t = temp_tree("bundle-fm-none");
        let index = t.join("index.md");
        touch(&index, "# plain\n");
        assert!(!declares_okf_version(&index));
        assert!(!declares_okf_version(&t.join("missing.md")));
        std::fs::remove_dir_all(&t).ok();
    }

    #[test]
    fn detect_root_on_own_knowledge_bundle() {
        // docs/knowledge/index.md declares okf_version, so rule 1 fires and
        // this holds even in a checkout without `.git`.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let file = manifest.join("docs/knowledge/tui/app-loop.md");
        let expected = fs::canonicalize(manifest.join("docs/knowledge")).unwrap();
        assert_eq!(detect_root(&file).unwrap(), expected);
    }

    #[test]
    fn detect_root_accepts_relative_path() {
        // cargo runs unit tests with cwd = the manifest dir.
        let root = detect_root(Path::new("docs/knowledge/index.md")).unwrap();
        assert!(root.is_absolute(), "got {}", root.display());
    }
}
