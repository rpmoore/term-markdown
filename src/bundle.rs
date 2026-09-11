//! OKF bundle-root detection.
//!
//! Absolute links (`/x/y.md`) in an OKF bundle are relative to the bundle
//! root, not the host filesystem, but nothing machine-readable declares
//! where that root is: the root `index.md` *may* carry `okf_version`
//! frontmatter, and most subdirectories have an `index.md` of their own.
//! These helpers implement the heuristic used when `--root` isn't given.
//! Pure filesystem lookups only — no terminal I/O.

use std::fs;
use std::io;
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
    let home = std::env::home_dir()?;
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

/// Whether `index` opens with a `---` frontmatter block that contains an
/// `okf_version:` key. An unreadable file, no opening fence, or an unclosed
/// fence (mirroring `markdown::strip_frontmatter`) all count as "no". The
/// file is already in memory, so the scan for the closing fence is unbounded
/// — a long `tags:` list must not hide the key.
fn declares_okf_version(index: &Path) -> bool {
    let Ok(source) = fs::read_to_string(index) else {
        return false;
    };
    let mut lines = source.lines().map(str::trim_end);
    if lines.next() != Some("---") {
        return false;
    }
    let mut found = false;
    for line in lines {
        if line == "---" {
            return found;
        }
        if line.trim_start().starts_with("okf_version:") {
            found = true;
        }
    }
    false
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
    fn declares_okf_version_false_when_fence_unclosed() {
        let t = temp_tree("bundle-fm-unclosed");
        let index = t.join("index.md");
        touch(&index, "---\nokf_version: \"0.2\"\n\n# never closed\n");
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
