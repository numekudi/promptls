//! Document text with LSP position <-> byte offset conversion.
//!
//! LSP positions are UTF-16 code-unit based (the default encoding). Prompts
//! frequently contain non-ASCII text (e.g. Japanese), so we must convert
//! properly instead of assuming byte == column.

use tower_lsp::lsp_types::Position;

/// An in-memory copy of an open document plus a line-start table for
/// O(log n) position lookups.
#[derive(Debug, Clone)]
pub struct Document {
    text: String,
    /// Byte offset of the first character of each line.
    line_starts: Vec<usize>,
}

impl Document {
    pub fn new(text: String) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { text, line_starts }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Convert a byte offset (must be on a char boundary) into an LSP position.
    pub fn offset_to_position(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        // Index of the last line start <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line];
        let col_utf16 = self.text[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position::new(line as u32, col_utf16 as u32)
    }

    /// Convert an LSP position into a byte offset. Returns `None` if the line is
    /// out of range; a column past the end of the line clamps to the line end.
    pub fn position_to_offset(&self, pos: Position) -> Option<usize> {
        let line_start = *self.line_starts.get(pos.line as usize)?;
        let line_end = self
            .line_starts
            .get(pos.line as usize + 1)
            .map(|s| s - 1) // exclude the '\n'
            .unwrap_or(self.text.len());
        let line_text = &self.text[line_start..line_end];

        let mut remaining = pos.character as usize;
        for (byte_idx, ch) in line_text.char_indices() {
            if remaining == 0 {
                return Some(line_start + byte_idx);
            }
            let units = ch.len_utf16();
            if remaining < units {
                // Position points inside a surrogate pair; snap to the char start.
                return Some(line_start + byte_idx);
            }
            remaining -= units;
        }
        Some(line_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_ascii() {
        let d = Document::new("abc\ndef".into());
        assert_eq!(d.offset_to_position(5), Position::new(1, 1));
        assert_eq!(d.position_to_offset(Position::new(1, 1)), Some(5));
    }

    #[test]
    fn multibyte_columns_are_utf16() {
        // "日本" = 2 chars, 6 bytes, 2 UTF-16 units. "😀" = 4 bytes, 2 units.
        let d = Document::new("日本😀x".into());
        assert_eq!(d.offset_to_position(6), Position::new(0, 2));
        assert_eq!(d.offset_to_position(10), Position::new(0, 4));
        assert_eq!(d.position_to_offset(Position::new(0, 4)), Some(10));
        // Column beyond line end clamps.
        assert_eq!(d.position_to_offset(Position::new(0, 99)), Some(11));
        // Line beyond document is None.
        assert_eq!(d.position_to_offset(Position::new(3, 0)), None);
    }

    #[test]
    fn trailing_newline_creates_empty_last_line() {
        let d = Document::new("a\n".into());
        assert_eq!(d.offset_to_position(2), Position::new(1, 0));
        assert_eq!(d.position_to_offset(Position::new(1, 0)), Some(2));
    }
}
