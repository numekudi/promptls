//! Project file index with fuzzy path search.
//!
//! Walks the project root honoring `.gitignore` (via the `ignore` crate, the
//! same engine ripgrep uses) and keeps a flat list of relative paths. Queries
//! are matched with `nucleo-matcher` against the whole relative path, so
//! `src/comp/but` finds `src/components/Button.tsx`.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// One indexed filesystem entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the index root, always `/`-separated, no trailing `/`.
    pub rel: String,
    pub is_dir: bool,
}

#[derive(Debug)]
pub struct FileIndex {
    root: PathBuf,
    entries: Vec<Entry>,
    /// True if the walk hit `max_entries` and stopped early.
    pub truncated: bool,
}

impl FileIndex {
    /// An empty index (used before the first build completes).
    pub fn empty(root: PathBuf) -> Self {
        Self { root, entries: Vec::new(), truncated: false }
    }

    /// Walk `root` and build the index. Hidden files are included (prompts
    /// often reference `.claude/` or `.github/`), but `.git` itself and anything
    /// git-ignored are skipped. Stops after `max_entries` entries.
    pub fn build(root: &Path, max_entries: usize) -> Self {
        let mut entries = Vec::new();
        let mut truncated = false;

        let walker = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|e| e.file_name() != ".git")
            .build();

        for dent in walker {
            let dent = match dent {
                Ok(d) => d,
                Err(err) => {
                    tracing::debug!("walk error: {err}");
                    continue;
                }
            };
            let Ok(rel_path) = dent.path().strip_prefix(root) else { continue };
            if rel_path.as_os_str().is_empty() {
                continue; // the root itself
            }
            let Some(rel) = rel_path.to_str() else {
                tracing::debug!("skipping non-UTF-8 path {:?}", rel_path);
                continue;
            };
            let is_dir = dent.file_type().is_some_and(|t| t.is_dir());
            entries.push(Entry { rel: rel.replace('\\', "/"), is_dir });
            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
        }

        Self { root: root.to_path_buf(), entries, truncated }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Fuzzy-search entries by `query`, best matches first, at most `limit`.
    ///
    /// An empty query lists root-level entries (directories first) so that a
    /// bare `@` behaves like a shell `ls`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Entry> {
        if query.is_empty() {
            let mut top: Vec<&Entry> = self.entries.iter().filter(|e| !e.rel.contains('/')).collect();
            top.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.rel.cmp(&b.rel)));
            top.truncate(limit);
            return top;
        }

        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(&Entry, u32)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let haystack = Utf32Str::new(&e.rel, &mut buf);
                pattern.score(haystack, &mut matcher).map(|s| (e, s))
            })
            .collect();
        // Higher score first; break ties by shorter path so `main.rs` beats
        // `deep/nested/main.rs` at equal score.
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.rel.len().cmp(&b.0.rel.len())));
        scored.into_iter().take(limit).map(|(e, _)| e).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("src/components")).unwrap();
        fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(r.join(".claude")).unwrap();
        fs::create_dir_all(r.join(".git")).unwrap();
        fs::write(r.join("src/components/Button.tsx"), "").unwrap();
        fs::write(r.join("src/main.rs"), "").unwrap();
        fs::write(r.join("node_modules/pkg/index.js"), "").unwrap();
        fs::write(r.join(".claude/settings.json"), "").unwrap();
        fs::write(r.join(".git/HEAD"), "").unwrap();
        fs::write(r.join(".gitignore"), "node_modules/\n").unwrap();
        fs::write(r.join("README.md"), "").unwrap();
        dir
    }

    #[test]
    fn respects_gitignore_and_skips_dot_git_but_keeps_hidden() {
        let dir = fixture();
        let idx = FileIndex::build(dir.path(), 10_000);
        let rels: Vec<&str> = idx.entries.iter().map(|e| e.rel.as_str()).collect();
        assert!(rels.contains(&"src/components/Button.tsx"));
        assert!(rels.contains(&".claude/settings.json"));
        assert!(!rels.iter().any(|r| r.starts_with("node_modules")));
        assert!(!rels.iter().any(|r| r.starts_with(".git/") || *r == ".git"));
    }

    #[test]
    fn fuzzy_search_matches_across_segments() {
        let dir = fixture();
        let idx = FileIndex::build(dir.path(), 10_000);
        let hits = idx.search("src/comp/but", 5);
        assert_eq!(hits[0].rel, "src/components/Button.tsx");
        let hits = idx.search("but", 5);
        assert_eq!(hits[0].rel, "src/components/Button.tsx");
    }

    #[test]
    fn empty_query_lists_root_dirs_first() {
        let dir = fixture();
        let idx = FileIndex::build(dir.path(), 10_000);
        let hits: Vec<(&str, bool)> = idx.search("", 50).iter().map(|e| (e.rel.as_str(), e.is_dir)).collect();
        assert!(hits.iter().all(|(r, _)| !r.contains('/')));
        let first_file = hits.iter().position(|(_, d)| !*d).unwrap();
        assert!(hits[..first_file].iter().all(|(_, d)| *d));
    }

    #[test]
    fn truncation_flag() {
        let dir = fixture();
        let idx = FileIndex::build(dir.path(), 2);
        assert!(idx.truncated);
        assert_eq!(idx.len(), 2);
    }
}
