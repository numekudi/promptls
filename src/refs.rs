//! Extraction of `@path` references from prompt text.
//!
//! Grammar (informal):
//!   - `@` must be at the start of the text or preceded by a boundary char
//!     (whitespace, brackets, quotes) so `user@example.com` is not a reference.
//!   - The path runs until whitespace or a terminator (brackets, quotes, `,` `|`).
//!   - Trailing sentence punctuation (`.` `,` `;` `:` `!` `?`) is stripped so
//!     "see @src/main.rs." refers to `src/main.rs`.
//!   - An optional `:LINE` or `:LINE-LINE` suffix is parsed as a line hint.

/// A resolved `@path` token in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    /// Byte offset of the `@` character.
    pub start: usize,
    /// Byte offset one past the last character of the reference (after
    /// trailing punctuation has been stripped).
    pub end: usize,
    /// The path text without `@` and without any `:LINE` suffix.
    pub path: String,
    /// 1-based line hint from a `:LINE` suffix, if present.
    pub line: Option<u32>,
}

/// The `@` token currently being typed at the cursor, used for completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Byte offset of the `@` character.
    pub at: usize,
    /// Text between `@` and the cursor.
    pub text: String,
}

/// Characters that end a path token.
fn is_terminator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '\'' | '`' | ',' | '|')
}

/// Whether a `@` preceded by `prev` starts a reference (vs. e.g. an email).
fn is_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'' | '`' | ',' | '|' | '*' | '_'),
    }
}

/// Punctuation that commonly follows a path in prose and is not part of it.
fn is_trailing_punct(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?')
}

/// Split an optional `:N` / `:N-M` suffix off a path.
fn split_line_suffix(token: &str) -> (&str, Option<u32>) {
    let Some(colon) = token.rfind(':') else {
        return (token, None);
    };
    let suffix = &token[colon + 1..];
    let first = suffix.split('-').next().unwrap_or("");
    if !first.is_empty()
        && first.bytes().all(|b| b.is_ascii_digit())
        && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        && let Ok(n) = first.parse::<u32>()
    {
        return (&token[..colon], Some(n));
    }
    (token, None)
}

/// Find all `@path` references in `text`.
pub fn find_refs(text: &str) -> Vec<PathRef> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    let mut iter = text.char_indices().peekable();

    while let Some((i, c)) = iter.next() {
        if c == '@' && is_boundary(prev) {
            // Scan forward to the raw end of the token.
            let mut end = text.len();
            for (j, cj) in text[i + 1..].char_indices() {
                if is_terminator(cj) {
                    end = i + 1 + j;
                    break;
                }
            }
            let mut raw = &text[i + 1..end];
            // Strip trailing prose punctuation.
            while let Some(last) = raw.chars().last() {
                if is_trailing_punct(last) {
                    raw = &raw[..raw.len() - last.len_utf8()];
                } else {
                    break;
                }
            }
            if !raw.is_empty() {
                let (path, line) = split_line_suffix(raw);
                if !path.is_empty() {
                    out.push(PathRef {
                        start: i,
                        end: i + 1 + raw.len(),
                        path: path.to_string(),
                        line,
                    });
                }
            }
            // Skip past the token so `@a@b` does not double-match.
            while let Some(&(j, _)) = iter.peek() {
                if j >= end {
                    break;
                }
                iter.next();
            }
            prev = Some(c);
            continue;
        }
        prev = Some(c);
    }
    out
}

/// The reference whose span contains `offset`, if any.
pub fn ref_at(text: &str, offset: usize) -> Option<PathRef> {
    find_refs(text)
        .into_iter()
        .find(|r| r.start <= offset && offset <= r.end)
}

/// The `@` token being typed immediately before `cursor`, if the cursor is
/// inside one (no terminator between the `@` and the cursor).
pub fn query_at(text: &str, cursor: usize) -> Option<Query> {
    let before = &text[..cursor];
    let at = before.rfind('@')?;
    let prev = before[..at].chars().next_back();
    if !is_boundary(prev) {
        return None;
    }
    let query = &before[at + 1..];
    if query.chars().any(is_terminator) {
        return None;
    }
    Some(Query { at, text: query.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<(String, Option<u32>)> {
        find_refs(text).into_iter().map(|r| (r.path, r.line)).collect()
    }

    #[test]
    fn basic_and_punctuation() {
        assert_eq!(paths("look at @src/main.rs."), vec![("src/main.rs".into(), None)]);
        assert_eq!(paths("(@a/b.ts) and @c/d,"), vec![("a/b.ts".into(), None), ("c/d".into(), None)]);
        assert_eq!(paths("`@x.md`"), vec![("x.md".into(), None)]);
    }

    #[test]
    fn email_and_lone_at_are_ignored() {
        assert!(paths("mail me@example.com").is_empty());
        assert!(paths("a @ b").is_empty());
        assert!(paths("@").is_empty());
    }

    #[test]
    fn line_suffix() {
        assert_eq!(paths("@src/a.rs:42"), vec![("src/a.rs".into(), Some(42))]);
        assert_eq!(paths("@src/a.rs:10-20"), vec![("src/a.rs".into(), Some(10))]);
        // Trailing colon is prose punctuation, not a line hint.
        assert_eq!(paths("@src/a.rs: yes"), vec![("src/a.rs".into(), None)]);
        // Non-numeric suffix stays part of the path.
        assert_eq!(paths("@C:foo"), vec![("C:foo".into(), None)]);
    }

    #[test]
    fn spans_and_ref_at() {
        let t = "x @a/b.rs. y";
        let r = &find_refs(t)[0];
        assert_eq!((r.start, r.end), (2, 9));
        assert_eq!(ref_at(t, 5).unwrap().path, "a/b.rs");
        assert_eq!(ref_at(t, 9).unwrap().path, "a/b.rs"); // cursor right after token
        assert!(ref_at(t, 10).is_none());
        assert!(ref_at(t, 0).is_none());
    }

    #[test]
    fn multibyte_before_reference() {
        let t = "見て @src/x.rs";
        let r = &find_refs(t)[0];
        assert_eq!(&t[r.start..r.end], "@src/x.rs");
    }

    #[test]
    fn query_detection() {
        assert_eq!(query_at("see @src/co", 11), Some(Query { at: 4, text: "src/co".into() }));
        assert_eq!(query_at("@", 1), Some(Query { at: 0, text: String::new() }));
        assert!(query_at("see @src/co foo", 15).is_none());
        assert!(query_at("me@ex", 5).is_none());
        assert!(query_at("plain", 5).is_none());
    }
}
