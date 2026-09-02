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

/// Collect the keys of the JSON object at `path`, in the order they appear in the document.
///
/// The key currently under the cursor is skipped — while typing, that key is not an
/// existing sibling but the one being completed.  Returns an empty vec when `path` does
/// not resolve to an object (malformed or still-being-typed input included).
///
/// This is a standalone forward scan rather than an extension of `position_to_context`:
/// keys *after* the cursor must be collected too, and the context scanner returns as soon
/// as it has classified the cursor.
pub fn object_keys_at(text: &str, path: &[PathSegment], line: u32, character: u32) -> Vec<String> {
    let cursor = lsp_position_to_byte_offset(text, line, character);
    let bytes = text.as_bytes();
    let mut pos = 0;

    skip_whitespace(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return Vec::new();
    }

    if !descend_to(bytes, &mut pos, path) {
        return Vec::new();
    }

    collect_keys(bytes, &mut pos, cursor)
}

/// Walk `pos` (parked on a `{` or `[`) down to the container named by `path`.
/// Returns false when the path does not resolve.
fn descend_to(bytes: &[u8], pos: &mut usize, path: &[PathSegment]) -> bool {
    let Some((segment, rest)) = path.split_first() else {
        return true;
    };

    match segment {
        PathSegment::Key(want) => {
            if *pos >= bytes.len() || bytes[*pos] != b'{' {
                return false;
            }
            *pos += 1; // consume '{'
            loop {
                skip_whitespace(bytes, pos);
                if *pos >= bytes.len() || bytes[*pos] == b'}' {
                    return false;
                }
                if bytes[*pos] == b',' {
                    *pos += 1;
                    continue;
                }
                if bytes[*pos] != b'"' {
                    // Malformed — give up rather than guess.
                    return false;
                }
                let key = scan_string(bytes, pos);
                skip_whitespace(bytes, pos);
                if *pos < bytes.len() && bytes[*pos] == b':' {
                    *pos += 1;
                }
                skip_whitespace(bytes, pos);
                if *pos >= bytes.len() {
                    return false;
                }
                if key == *want {
                    return descend_to(bytes, pos, rest);
                }
                skip_value(bytes, pos);
            }
        }
        PathSegment::Index(want) => {
            if *pos >= bytes.len() || bytes[*pos] != b'[' {
                return false;
            }
            *pos += 1; // consume '['
            let mut index = 0usize;
            loop {
                skip_whitespace(bytes, pos);
                if *pos >= bytes.len() || bytes[*pos] == b']' {
                    return false;
                }
                if bytes[*pos] == b',' {
                    *pos += 1;
                    index += 1;
                    continue;
                }
                if index == *want {
                    return descend_to(bytes, pos, rest);
                }
                skip_value(bytes, pos);
            }
        }
    }
}

/// List the keys of the object starting at `pos`, skipping the one spanning `cursor`.
fn collect_keys(bytes: &[u8], pos: &mut usize, cursor: Option<usize>) -> Vec<String> {
    let mut keys = Vec::new();

    if *pos >= bytes.len() || bytes[*pos] != b'{' {
        return keys;
    }
    *pos += 1; // consume '{'

    loop {
        skip_whitespace(bytes, pos);
        if *pos >= bytes.len() {
            break;
        }
        match bytes[*pos] {
            b'}' => break,
            b',' => {
                *pos += 1;
                continue;
            }
            b'"' => {}
            _ => {
                // Malformed token between entries — skip it and keep scanning.
                *pos += 1;
                continue;
            }
        }

        let key_start = *pos;
        let key = scan_string(bytes, pos);
        let key_end = *pos;

        // The key being typed is not an existing sibling.
        let under_cursor = cursor.is_some_and(|c| c >= key_start && c <= key_end);
        if !under_cursor {
            keys.push(key);
        }

        skip_whitespace(bytes, pos);
        if *pos < bytes.len() && bytes[*pos] == b':' {
            *pos += 1;
            skip_whitespace(bytes, pos);
            skip_value(bytes, pos);
        }
    }

    keys
}

/// Advance `pos` past one complete JSON value (object, array, string, or literal).
fn skip_value(bytes: &[u8], pos: &mut usize) {
    if *pos >= bytes.len() {
        return;
    }
    match bytes[*pos] {
        b'{' | b'[' => {
            let open = bytes[*pos];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            while *pos < bytes.len() {
                match bytes[*pos] {
                    b'"' => {
                        scan_string(bytes, pos);
                        continue;
                    }
                    c if c == open => depth += 1,
                    c if c == close => {
                        depth -= 1;
                        if depth == 0 {
                            *pos += 1;
                            return;
                        }
                    }
                    _ => {}
                }
                *pos += 1;
            }
        }
        b'"' => {
            scan_string(bytes, pos);
        }
        _ => skip_literal(bytes, pos),
    }
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

    // ── object_keys_at ──────────────────────────────────────────

    /// Line 3 holds a bare `""` — the key being typed.
    const TYPING: &str = r#"{
  "$schema": "s",
  "name": "hello",
  "",
  "count": 42,
  "meta": { "author": "me" }
}"#;

    #[test]
    fn test_object_keys_in_document_order() {
        // Cursor between the quotes of the empty key on line 3.
        let keys = object_keys_at(TYPING, &[], 3, 3);
        assert_eq!(
            keys,
            vec!["$schema", "name", "count", "meta"],
            "Keys must come back in document order, not sorted"
        );
    }

    #[test]
    fn test_object_keys_excludes_key_under_cursor() {
        // Cursor inside the existing "name" key (line 2, col 4) — it is being edited,
        // so it must not count as an existing sibling.
        let keys = object_keys_at(TYPING, &[], 2, 4);
        assert!(
            !keys.contains(&"name".to_owned()),
            "Key under the cursor must be excluded, got: {keys:?}"
        );
        assert!(
            keys.contains(&"count".to_owned()),
            "Other keys must still be collected, got: {keys:?}"
        );
    }

    #[test]
    fn test_object_keys_nested_path() {
        // Line 5 is `  "meta": { "author": "me" }` — col 11 is the space after `{`,
        // i.e. inside the nested object but not on its key.
        let keys = object_keys_at(TYPING, &[PathSegment::Key("meta".into())], 5, 11);
        assert_eq!(keys, vec!["author"]);
    }

    #[test]
    fn test_object_keys_through_array_index() {
        let text = r#"{"list": [{"a": 1, "b": 2}, {"c": 3}]}"#;
        let path = vec![PathSegment::Key("list".into()), PathSegment::Index(0)];
        assert_eq!(object_keys_at(text, &path, 0, 0), vec!["a", "b"]);

        let path = vec![PathSegment::Key("list".into()), PathSegment::Index(1)];
        assert_eq!(object_keys_at(text, &path, 0, 0), vec!["c"]);
    }

    #[test]
    fn test_object_keys_skips_nested_objects() {
        // Keys of an inner object must not leak into the outer object's key list.
        let text = r#"{"a": {"inner": 1}, "b": [1, 2], "c": 3}"#;
        assert_eq!(object_keys_at(text, &[], 0, 0), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_object_keys_unresolvable_path_is_empty() {
        assert!(object_keys_at(TYPING, &[PathSegment::Key("nope".into())], 0, 0).is_empty());
        // `name` is a string, not an object.
        assert!(object_keys_at(TYPING, &[PathSegment::Key("name".into())], 0, 0).is_empty());
        // Not an object at all.
        assert!(object_keys_at("[1, 2]", &[], 0, 0).is_empty());
    }

    #[test]
    fn test_object_keys_survives_malformed_input() {
        // Unterminated document mid-typing must not hang or panic.
        let text = "{\n  \"a\": 1,\n  \"b\": {\n    \"c\":\n";
        let keys = object_keys_at(text, &[], 3, 8);
        assert!(keys.contains(&"a".to_owned()), "got: {keys:?}");
    }
}
