use crate::document::DocumentStore;
use crate::position::{object_keys_at, position_to_context, PathSegment, PositionContext};
use crate::schema::{SchemaCache, SchemaNode};
use std::sync::Arc;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind,
};
use tracing::debug;

pub async fn handle_completion(
    documents: &Arc<DocumentStore>,
    schema_cache: &Arc<SchemaCache>,
    params: CompletionParams,
) -> Option<CompletionResponse> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;

    let text = documents.get_text(uri)?;
    let schema_url = documents.get_schema_url(uri)?;

    let context = position_to_context(&text, pos.line, pos.character);
    debug!("Completion context: {context:?}");

    let schema_value = schema_cache.get_or_fetch(&schema_url).await.ok()?;
    let root_node = SchemaNode::new(&schema_value, &schema_value);

    let items = match &context {
        PositionContext::Key { path } => {
            // Cursor is inside an existing quoted key (e.g. between autopairs "").
            // `path` now includes the key itself; drop the last segment to get the parent.
            // insert_text must NOT include a leading '"' — the opening quote is already there.
            let parent_path = if path.is_empty() {
                &[][..]
            } else {
                &path[..path.len() - 1]
            };
            let parent_node = if parent_path.is_empty() {
                SchemaNode::new(&schema_value, &schema_value)
            } else {
                root_node.navigate(parent_path)?
            };
            let existing = object_keys_at(&text, parent_path, pos.line, pos.character);
            let names = order_by_absence(parent_node.property_names(), &existing);
            debug!(
                "Completion Key: found {} property names at parent {parent_path:?}, \
                 {} already present in the document",
                names.len(),
                existing.len()
            );
            property_completions_from_names(names, &parent_node, false)
        }

        PositionContext::KeyStart { path } => {
            // Cursor is at the opening '"' of a key — include it in insert_text.
            let parent_node = if path.is_empty() {
                SchemaNode::new(&schema_value, &schema_value)
            } else {
                root_node.navigate(path)?
            };
            let existing = object_keys_at(&text, path, pos.line, pos.character);
            let names = order_by_absence(parent_node.property_names(), &existing);
            debug!(
                "Completion KeyStart: found {} property names at path {path:?}, \
                 {} already present in the document",
                names.len(),
                existing.len()
            );
            property_completions_from_names(names, &parent_node, true)
        }

        PositionContext::Value { path } | PositionContext::ValueStart { path } => {
            // Suggest enum values or type-based snippets for the value position
            let node = root_node.navigate(path)?;
            value_completions(&node)
        }

        PositionContext::Unknown => {
            debug!("Completion: Unknown context, returning None");
            return None;
        }
    };

    if items.is_empty() {
        return None;
    }

    Some(CompletionResponse::Array(items))
}

/// Order property suggestions by what the object at the cursor is still missing.
///
/// Properties absent from the object come first — those are the ones worth offering —
/// keeping the schema's own (alphabetical) order among them.  Properties already present
/// follow, in the order they appear in the document, so re-typing an existing key lands
/// where the reader expects it.
fn order_by_absence(names: Vec<String>, existing: &[String]) -> Vec<String> {
    let (mut present, missing): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|n| existing.contains(n));

    present.sort_by_key(|n| existing.iter().position(|k| k == n).unwrap_or(usize::MAX));

    let mut ordered = missing;
    ordered.extend(present);
    ordered
}

fn property_completions_from_names(
    names: Vec<String>,
    node: &SchemaNode,
    include_leading_quote: bool,
) -> Vec<CompletionItem> {
    names
        .into_iter()
        .enumerate()
        .map(|(rank, name)| {
            let info = node
                .navigate(&[PathSegment::Key(name.clone())])
                .map(|n| n.hover_info());

            let detail = info.as_ref().and_then(|i| i.type_info.clone());
            let documentation = info.and_then(|i| {
                i.description.map(|d| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: d,
                    })
                })
            });

            // When cursor is inside existing quotes (Key context), the opening '"' is
            // already in the buffer — autopairs inserts it. Only add it when the cursor
            // sits at the quote itself (KeyStart context).
            let insert_text = if include_leading_quote {
                format!("\"{name}\": ")
            } else {
                format!("{name}\": ")
            };

            CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                detail,
                documentation,
                insert_text: Some(insert_text),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                // Clients sort by sortText, not by array order — the ranking from
                // `order_by_absence` only survives if it is spelled out here.
                sort_text: Some(format!("{rank:05}")),
                ..Default::default()
            }
        })
        .collect()
}

fn value_completions(node: &SchemaNode) -> Vec<CompletionItem> {
    let enum_values = node.enum_values();
    if !enum_values.is_empty() {
        return enum_values
            .into_iter()
            .map(|val| CompletionItem {
                label: val.clone(),
                kind: Some(CompletionItemKind::VALUE),
                insert_text: Some(val),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            })
            .collect();
    }

    // Type-based snippets
    match node.schema_type() {
        Some("boolean") => vec![make_snippet("true", "true"), make_snippet("false", "false")],
        Some("null") => vec![make_snippet("null", "null")],
        Some("array") => vec![make_snippet("[]", "[$1]")],
        Some("object") => vec![make_snippet("{}", "{$1}")],
        Some("string") => vec![make_snippet("\"\"", "\"$1\"")],
        _ => vec![],
    }
}

fn make_snippet(label: &str, insert_text: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::VALUE),
        insert_text: Some(insert_text.to_owned()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn test_missing_properties_come_first() {
        // Schema offers these (alphabetical, as `property_names` returns them);
        // the document already has "count" and "name".
        let ordered = order_by_absence(
            names(&["count", "enabled", "meta", "name"]),
            &names(&["name", "count"]),
        );
        assert_eq!(ordered, names(&["enabled", "meta", "name", "count"]));
    }

    #[test]
    fn test_present_properties_keep_document_order() {
        // Document order is the reverse of alphabetical — the tail must follow the
        // document, not the schema.
        let ordered = order_by_absence(
            names(&["a", "b", "c"]),
            &names(&["c", "b", "unrelated", "a"]),
        );
        assert_eq!(ordered, names(&["c", "b", "a"]));
    }

    #[test]
    fn test_empty_object_keeps_schema_order() {
        let ordered = order_by_absence(names(&["a", "b", "c"]), &[]);
        assert_eq!(ordered, names(&["a", "b", "c"]));
    }

    #[test]
    fn test_sort_text_encodes_the_ranking() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "count": {}, "enabled": {}, "name": {} }
        });
        let node = SchemaNode::new(&schema, &schema);
        let ordered = order_by_absence(node.property_names(), &names(&["name", "count"]));
        let items = property_completions_from_names(ordered, &node, true);

        let mut by_sort_text = items.clone();
        by_sort_text.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        let labels: Vec<&str> = by_sort_text.iter().map(|i| i.label.as_str()).collect();

        assert_eq!(
            labels,
            vec!["enabled", "name", "count"],
            "sortText must reproduce the ranking client-side"
        );
        assert!(
            items.iter().all(|i| i.sort_text.is_some()),
            "every property item needs a sortText"
        );
    }
}
