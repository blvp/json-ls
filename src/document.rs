use anyhow::{anyhow, Result};
use dashmap::DashMap;
use ropey::Rope;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

pub struct DocumentState {
    pub rope: Rope,
    pub version: i32,
    pub schema_url: Option<String>,
    pub text: String,
}

pub struct DocumentStore {
    inner: DashMap<Url, DocumentState>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn open(&self, uri: Url, version: i32, text: String) {
        let schema_url = extract_schema_url(&text);
        let rope = Rope::from_str(&text);
        self.inner.insert(
            uri,
            DocumentState {
                rope,
                version,
                schema_url,
                text,
            },
        );
    }

    /// Apply incremental or full text changes from a `did_change` notification.
    pub fn update(
        &self,
        uri: &Url,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<()> {
        let mut state = self
            .inner
            .get_mut(uri)
            .ok_or_else(|| anyhow!("Document not found: {uri}"))?;

        for change in changes {
            match change.range {
                None => {
                    // Full replacement
                    state.rope = Rope::from_str(&change.text);
                    state.text = change.text;
                }
                Some(range) => {
                    // Incremental update — convert LSP range to rope char indices
                    let start = lsp_pos_to_char_idx(&state.rope, range.start)?;
                    let end = lsp_pos_to_char_idx(&state.rope, range.end)?;
                    state.rope.remove(start..end);
                    state.rope.insert(start, &change.text);
                    // Rebuild text from rope for diagnostics
                    state.text = state.rope.to_string();
                }
            }
        }

        state.version = version;
        state.schema_url = extract_schema_url(&state.text);
        Ok(())
    }

    pub fn close(&self, uri: &Url) {
        self.inner.remove(uri);
    }

    pub fn get_schema_url(&self, uri: &Url) -> Option<String> {
        self.inner.get(uri)?.schema_url.clone()
    }

    pub fn get_text(&self, uri: &Url) -> Option<String> {
        self.inner.get(uri).map(|s| s.text.clone())
    }

    // TODO: use this in a future `textDocument/formatting` handler — a rope reference is
    // needed to efficiently apply formatter edits back as incremental LSP text edits.
    #[allow(dead_code)]
    pub fn get_rope(&self, uri: &Url) -> Option<Rope> {
        self.inner.get(uri).map(|s| s.rope.clone())
    }
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert an LSP `Position` (0-based line + UTF-16 character) to a ropey char index.
pub fn lsp_pos_to_char_idx(rope: &Rope, pos: tower_lsp::lsp_types::Position) -> Result<usize> {
    let line = pos.line as usize;
    if line >= rope.len_lines() {
        return Err(anyhow!(
            "Line {line} out of range (doc has {} lines)",
            rope.len_lines()
        ));
    }

    let line_char_start = rope.line_to_char(line);
    let line_slice = rope.line(line);

    // Count UTF-16 code units to find the correct char offset within the line
    let col_utf16 = pos.character as usize;
    let mut utf16_remaining = col_utf16;
    let mut char_offset = 0;

    for ch in line_slice.chars() {
        if utf16_remaining == 0 {
            break;
        }
        let utf16_len = ch.len_utf16();
        if utf16_remaining < utf16_len {
            // Cursor is in the middle of a surrogate pair — snap to start
            break;
        }
        utf16_remaining -= utf16_len;
        char_offset += 1;
    }

    Ok(line_char_start + char_offset)
}

/// Scan the first ~2 KiB of the document for a `"$schema"` key.
pub fn extract_schema_url(text: &str) -> Option<String> {
    // We only need to look near the top of the file
    let scan = &text[..text.len().min(2048)];

    // Find "$schema" key
    let key_pos = scan.find("\"$schema\"")?;
    let after_key = &scan[key_pos + 9..]; // skip `"$schema"`

    // Find ':'
    let colon = after_key.find(':')? + 1;
    let after_colon = after_key[colon..].trim_start();

    // Expect a quoted string value
    if !after_colon.starts_with('"') {
        return None;
    }

    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    let url = &inner[..end];

    if url.is_empty() {
        None
    } else {
        Some(url.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_schema_url() {
        let text = r#"{
  "$schema": "https://json-schema.org/draft-07/schema",
  "name": "test"
}"#;
        let url = extract_schema_url(text);
        assert_eq!(
            url,
            Some("https://json-schema.org/draft-07/schema".to_owned())
        );
    }

    #[test]
    fn test_extract_schema_url_missing() {
        let text = r#"{ "name": "test" }"#;
        assert!(extract_schema_url(text).is_none());
    }

    #[test]
    fn test_lsp_pos_to_char_ascii() {
        let rope = Rope::from_str("hello\nworld\n");
        let pos = tower_lsp::lsp_types::Position {
            line: 1,
            character: 3,
        };
        let idx = lsp_pos_to_char_idx(&rope, pos).unwrap();
        // line 1 starts at char 6 ("hello\n"), offset 3 → char 9
        assert_eq!(idx, 9);
    }

    #[test]
    fn test_lsp_pos_to_char_emoji() {
        // Emoji "😀" is 2 UTF-16 code units but 1 char
        let rope = Rope::from_str("a😀b\n");
        let pos = tower_lsp::lsp_types::Position {
            line: 0,
            character: 3,
        }; // after emoji
        let idx = lsp_pos_to_char_idx(&rope, pos).unwrap();
        assert_eq!(idx, 2); // 'a' + '😀' = 2 chars
    }
}
