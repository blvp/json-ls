use crate::document::DocumentStore;
use crate::schema::SchemaCache;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};
use tracing::{debug, warn};

/// Validate the document at `uri` against its declared `$schema`.
/// Returns an empty list if no schema is found, the document cannot be parsed,
/// or the schema cannot be fetched.
pub async fn validate_document(
    uri: &Url,
    documents: &Arc<DocumentStore>,
    schema_cache: &Arc<SchemaCache>,
) -> Result<Vec<Diagnostic>> {
    let Some(text) = documents.get_text(uri) else {
        return Ok(vec![]);
    };

    let Some(schema_url) = documents.get_schema_url(uri) else {
        debug!("No $schema for {uri}");
        return Ok(vec![]);
    };

    let schema_value = match schema_cache.get_or_fetch(&schema_url).await {
        Ok(v) => v,
        Err(e) => {
            warn!("Could not fetch schema {schema_url}: {e}");
            return Ok(vec![]);
        }
    };

    let instance: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            // Return a single syntax-error diagnostic
            let (line, col) = parse_error_position(&e, &text);
            return Ok(vec![Diagnostic {
                range: Range {
                    start: Position {
                        line,
                        character: col,
                    },
                    end: Position {
                        line,
                        character: col + 1,
                    },
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("json-syntax".into())),
                source: Some("json-ls".into()),
                message: format!("JSON syntax error: {e}"),
                ..Default::default()
            }]);
        }
    };

    let validator = match jsonschema::validator_for(&schema_value) {
        Ok(v) => v,
        Err(e) => {
            warn!("Could not compile schema {schema_url}: {e}");
            return Ok(vec![]);
        }
    };

    let mut diagnostics = Vec::new();

    for error in validator.iter_errors(&instance) {
        let path_str = error.instance_path().to_string();
        let range = instance_path_to_range(&path_str, &text);

        diagnostics.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String("schema-validation".into())),
            source: Some("json-ls".into()),
            message: error.to_string(),
            ..Default::default()
        });
    }

    debug!("Validated {uri}: {} error(s)", diagnostics.len());

    Ok(diagnostics)
}

/// Best-effort conversion of a JSON Pointer path (e.g. "/name/0") to an LSP Range
/// by scanning the document text for the matching location.
fn instance_path_to_range(path: &str, text: &str) -> Range {
    // If we can locate the field in the document, return a precise range.
    // Otherwise fall back to the top of the document.
    if let Some(range) = try_locate_path(path, text) {
        return range;
    }

    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 1,
        },
    }
}

/// Attempt to locate a JSON Pointer path in the raw text.
/// Only handles simple single-level key lookups for now.
fn try_locate_path(path: &str, text: &str) -> Option<Range> {
    // Only handle simple paths like "/key" for now
    let key = path.trim_start_matches('/').split('/').next()?;
    if key.is_empty() {
        return None;
    }

    // Try to find `"key":` pattern
    let needle = format!("\"{}\"", key);
    let start_byte = text.find(&needle)?;

    let (line, character) = byte_offset_to_lsp_pos(text, start_byte);
    let end_character = character + needle.len() as u32;

    Some(Range {
        start: Position { line, character },
        end: Position {
            line,
            character: end_character,
        },
    })
}

/// Convert a byte offset in `text` to an LSP Position (UTF-16 based).
pub fn byte_offset_to_lsp_pos(text: &str, byte_offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut line_start = 0usize;

    for (i, ch) in text.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }

    // Count UTF-16 units from line_start to byte_offset
    let col_text = &text[line_start..byte_offset.min(text.len())];
    let character = col_text.chars().map(|c| c.len_utf16() as u32).sum::<u32>();

    (line, character)
}

/// Extract line/column from a serde_json error message (best effort).
fn parse_error_position(e: &serde_json::Error, _text: &str) -> (u32, u32) {
    let line = e.line().saturating_sub(1) as u32;
    let col = e.column().saturating_sub(1) as u32;
    (line, col)
}
