/// A segment in a JSON path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// The semantic context of the cursor position within a JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionContext {
    /// Cursor is on/in a key string.  `path` is the full path TO this key (same semantics as `Value`).
    Key { path: Vec<PathSegment> },
    /// Cursor is just at the start of a key (e.g., at `"`).
    KeyStart { path: Vec<PathSegment> },
    /// Cursor is inside a value at `path`.
    Value { path: Vec<PathSegment> },
    /// Cursor is at the start position of a value (e.g., between `:` and value).
    ValueStart { path: Vec<PathSegment> },
    /// Position could not be classified (e.g., in whitespace at top-level).
    Unknown,
}

impl PositionContext {
    /// Return the JSON path this context refers to.
    // TODO: expose to future handlers (code actions, go-to-definition) that need
    // to extract the path from an already-computed PositionContext without re-scanning.
    #[allow(dead_code)]
    pub fn path(&self) -> &[PathSegment] {
        match self {
            PositionContext::Key { path }
            | PositionContext::KeyStart { path }
            | PositionContext::Value { path }
            | PositionContext::ValueStart { path } => path,
            PositionContext::Unknown => &[],
        }
    }
}

/// Convert an LSP `Position` (0-based line + UTF-16 char) to a byte offset in `text`.
fn lsp_position_to_byte_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut current_line = 0u32;
    let mut line_start = 0;

    for (i, ch) in text.char_indices() {
        if current_line == line {
            line_start = i;
            break;
        }
        if ch == '\n' {
            current_line += 1;
        }
        if current_line > line {
            return None;
        }
    }

    // Edge case: cursor is on the last line with no trailing newline
    if current_line != line {
        if current_line + 1 == line && !text.is_empty() {
            line_start = text.len();
        } else {
            return None;
        }
    }

    // Walk UTF-16 units from line_start
    let line_text = &text[line_start..];
    let mut utf16_count = 0u32;
    for (byte_off, ch) in line_text.char_indices() {
        if utf16_count >= character {
            return Some(line_start + byte_off);
        }
        utf16_count += ch.len_utf16() as u32;
    }

    // Cursor at end of line
    Some(line_start + line_text.len())
}

/// Scan `text` and determine the JSON context at the given byte target offset.
pub fn position_to_context(text: &str, line: u32, character: u32) -> PositionContext {
    let target = match lsp_position_to_byte_offset(text, line, character) {
        Some(t) => t,
        None => return PositionContext::Unknown,
    };

    let bytes = text.as_bytes();
    let mut pos = 0;

    // Skip leading whitespace and look for '{'
    skip_whitespace(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return PositionContext::Unknown;
    }

    let mut path: Vec<PathSegment> = Vec::new();
    let mut result = PositionContext::Unknown;

    scan_object(bytes, &mut pos, &mut path, target, &mut result);
    result
}

// ────────────────────────────────────────────────────────────
// Recursive-descent scanner
// ────────────────────────────────────────────────────────────

fn scan_object(
    bytes: &[u8],
    pos: &mut usize,
    path: &mut Vec<PathSegment>,
    target: usize,
    result: &mut PositionContext,
) {
    // Consume '{'
    *pos += 1;

    loop {
        skip_whitespace(bytes, pos);
        if *pos >= bytes.len() {
            break;
        }

        let ch = bytes[*pos];

        if ch == b'}' {
            *pos += 1;
            break;
        }

        if ch == b',' {
            *pos += 1;
            continue;
        }

        // At a key
        if ch == b'"' {
            // Check if target is at the opening quote (KeyStart)
            if target == *pos {
                *result = PositionContext::KeyStart { path: path.clone() };
                return;
            }

            let key_start = *pos;
            let key = scan_string(bytes, pos);

            // Check if target is inside the key string.
            // Include the key itself in the path so hover navigates to this field's schema.
            if target > key_start && target <= *pos {
                let mut key_path = path.clone();
                key_path.push(PathSegment::Key(key.clone()));
                *result = PositionContext::Key { path: key_path };
                return;
            }

            // After key, skip whitespace and ':'
            skip_whitespace(bytes, pos);
            if *pos >= bytes.len() {
                break;
            }
            if bytes[*pos] == b':' {
                *pos += 1;
            }
            skip_whitespace(bytes, pos);

            if *pos >= bytes.len() {
                break;
            }

            // Check if target is between ':' and the value, or exactly at value start
            if target > key_start && target <= *pos {
                let mut value_path = path.clone();
                value_path.push(PathSegment::Key(key.clone()));
                *result = PositionContext::ValueStart { path: value_path };
                return;
            }

            path.push(PathSegment::Key(key));
            scan_value(bytes, pos, path, target, result);

            if *result != PositionContext::Unknown {
                path.pop();
                return;
            }

            path.pop();
        } else {
            // Malformed — skip until next ',' or '}'
            *pos += 1;
        }
    }
}

fn scan_array(
    bytes: &[u8],
    pos: &mut usize,
    path: &mut Vec<PathSegment>,
    target: usize,
    result: &mut PositionContext,
) {
    // Consume '['
    *pos += 1;

    let mut index = 0usize;

    loop {
        skip_whitespace(bytes, pos);
        if *pos >= bytes.len() {
            break;
        }

        let ch = bytes[*pos];

        if ch == b']' {
            *pos += 1;
            break;
        }

        if ch == b',' {
            *pos += 1;
            index += 1;
            continue;
        }

        if target == *pos {
            let mut value_path = path.clone();
            value_path.push(PathSegment::Index(index));
            *result = PositionContext::ValueStart { path: value_path };
            return;
        }

        path.push(PathSegment::Index(index));
        scan_value(bytes, pos, path, target, result);
        if *result != PositionContext::Unknown {
            path.pop();
            return;
        }
        path.pop();
    }
}

fn scan_value(
    bytes: &[u8],
    pos: &mut usize,
    path: &mut Vec<PathSegment>,
    target: usize,
    result: &mut PositionContext,
) {
    if *pos >= bytes.len() {
        return;
    }

    match bytes[*pos] {
        b'{' => {
            let brace_pos = *pos;
            if target == brace_pos {
                *result = PositionContext::ValueStart { path: path.clone() };
                return;
            }
            scan_object(bytes, pos, path, target, result);
        }
        b'[' => {
            let bracket_pos = *pos;
            if target == bracket_pos {
                *result = PositionContext::ValueStart { path: path.clone() };
                return;
            }
            scan_array(bytes, pos, path, target, result);
        }
        b'"' => {
            let str_start = *pos;
            let _ = scan_string(bytes, pos);
            let str_end = *pos;

            if target >= str_start && target <= str_end {
                *result = PositionContext::Value { path: path.clone() };
            }
        }
        _ => {
            // number, true, false, null
            let lit_start = *pos;
            skip_literal(bytes, pos);
            let lit_end = *pos;

            if target >= lit_start && target <= lit_end {
                *result = PositionContext::Value { path: path.clone() };
            }
        }
    }
}

// ────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────

fn skip_whitespace(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\r' | b'\n') {
        *pos += 1;
    }
}

/// Consume a JSON string (including surrounding quotes), returning the unescaped content.
fn scan_string(bytes: &[u8], pos: &mut usize) -> String {
    let mut s = String::new();

    if *pos >= bytes.len() || bytes[*pos] != b'"' {
        return s;
    }
    *pos += 1; // skip opening '"'

    while *pos < bytes.len() {
        let ch = bytes[*pos];
        if ch == b'"' {
            *pos += 1; // skip closing '"'
            break;
        }
        if ch == b'\\' {
            *pos += 1; // skip backslash
            if *pos < bytes.len() {
                match bytes[*pos] {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => {
                        // Skip 4 hex digits; we don't fully decode escapes for key matching
                        s.push('?');
                        *pos += 1;
                        for _ in 0..3 {
                            if *pos < bytes.len() {
                                *pos += 1;
                            }
                        }
                        continue;
                    }
                    other => s.push(other as char),
                }
                *pos += 1;
            }
        } else {
            s.push(ch as char);
            *pos += 1;
        }
    }

    s
}

/// Skip over a literal (number, true, false, null).
fn skip_literal(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len()
        && !matches!(
            bytes[*pos],
            b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n'
        )
    {
        *pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(text: &str, line: u32, character: u32) -> PositionContext {
        position_to_context(text, line, character)
    }

    const DOC: &str = r#"{
  "$schema": "https://example.com/schema.json",
  "name": "hello",
  "count": 42,
  "tags": ["a", "b"],
  "nested": {
    "inner": true
  }
}"#;

    #[test]
    fn test_cursor_in_key() {
        // "$schema" key is on line 1: `  "$schema": ...`
        // The key starts at column 2 (0-indexed), cursor at col 4 → inside key
        let result = ctx(DOC, 1, 4);
        assert!(
            matches!(
                result,
                PositionContext::Key { .. } | PositionContext::KeyStart { .. }
            ),
            "Expected Key/KeyStart, got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_key_includes_key_in_path() {
        // Line 2: `  "name": "hello",`  cursor at col 4 → inside "name" key
        // Key { path } must include "name" so hover navigates to the field's schema.
        let result = ctx(DOC, 2, 4);
        assert!(
            matches!(result, PositionContext::Key { ref path } if *path == vec![PathSegment::Key("name".into())]),
            "Expected Key with path [name], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_nested_key_includes_full_path() {
        // Line 6: `    "inner": true`  cursor at col 6 → inside "inner" key
        // Key { path } must be [nested, inner] — the full path to the field.
        let result = ctx(DOC, 6, 6);
        assert!(
            matches!(result, PositionContext::Key { ref path } if *path == vec![
                PathSegment::Key("nested".into()),
                PathSegment::Key("inner".into())
            ]),
            "Expected Key with path [nested, inner], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_string_value() {
        // Line 2: `  "name": "hello",`
        // Value "hello" starts at column 10; cursor at col 12 → inside value
        let result = ctx(DOC, 2, 12);
        assert!(
            matches!(result, PositionContext::Value { ref path } if *path == vec![PathSegment::Key("name".into())]),
            "Expected Value at [name], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_number_value() {
        // Line 3: `  "count": 42,`
        // "count" value starts at col 11; cursor at col 12 → inside value
        let result = ctx(DOC, 3, 12);
        assert!(
            matches!(result, PositionContext::Value { ref path } if *path == vec![PathSegment::Key("count".into())]),
            "Expected Value at [count], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_nested_value() {
        // Line 6: `    "inner": true`
        // "inner" path should be [nested, inner]
        let result = ctx(DOC, 6, 14);
        assert!(
            matches!(result, PositionContext::Value { ref path } if *path == vec![
                PathSegment::Key("nested".into()),
                PathSegment::Key("inner".into())
            ]),
            "Expected Value at [nested, inner], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_in_array_item() {
        // Line 4: `  "tags": ["a", "b"],`
        // "a" is at approximately col 12
        let result = ctx(DOC, 4, 13);
        assert!(
            matches!(result, PositionContext::Value { ref path } if *path == vec![
                PathSegment::Key("tags".into()),
                PathSegment::Index(0)
            ]),
            "Expected Value at [tags, 0], got {result:?}"
        );
    }

    #[test]
    fn test_cursor_between_colon_and_value() {
        // Line 2: `  "name": "hello",`
        //                   ^ col 9 (after ':') → ValueStart at path [name]
        let result = ctx(DOC, 2, 9);
        // Between ':' and value, expect ValueStart or Value
        assert!(
            matches!(
                result,
                PositionContext::ValueStart { .. } | PositionContext::Value { .. }
            ),
            "Expected ValueStart or Value, got {result:?}"
        );
    }

    #[test]
    fn test_utf16_offset_with_multibyte() {
        // "😀" occupies 2 UTF-16 code units; cursor at character=3 should be past the emoji
        let text = "{\n  \"k\": \"😀x\"\n}";
        // Line 1: `  "k": "😀x"` — x is at UTF-16 col 10 (2+1+2+2+2+1=10)
        let result = ctx(text, 1, 10);
        assert!(
            matches!(result, PositionContext::Value { .. }),
            "Expected Value context for UTF-16 position, got {result:?}"
        );
    }

    #[test]
    fn test_key_start_at_quote() {
        // Cursor exactly at the opening quote of a key
        let text = "{\n  \"name\": \"v\"\n}";
        // Line 1, col 2 → opening '"' of "name"
        let result = ctx(text, 1, 2);
        assert!(
            matches!(
                result,
                PositionContext::KeyStart { .. } | PositionContext::Key { .. }
            ),
            "Expected KeyStart at opening quote, got {result:?}"
        );
    }

    #[test]
    fn test_empty_object() {
        let text = "{}";
        let result = ctx(text, 0, 1);
        // Inside empty object — Unknown or ValueStart is fine
        let _ = result; // just shouldn't panic
    }
}
