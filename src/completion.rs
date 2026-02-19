use crate::document::DocumentStore;
use crate::position::{position_to_context, PathSegment, PositionContext};
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
            // insert_text must NOT include a leading '"' — the opening quote is already there.
            let parent_node = if path.is_empty() {
                SchemaNode::new(&schema_value, &schema_value)
            } else {
                root_node.navigate(path)?
            };
            let names = parent_node.property_names();
            debug!(
                "Completion Key: found {} property names at path {path:?}",
                names.len()
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
            let names = parent_node.property_names();
            debug!(
                "Completion KeyStart: found {} property names at path {path:?}",
                names.len()
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

fn property_completions_from_names(
    names: Vec<String>,
    node: &SchemaNode,
    include_leading_quote: bool,
) -> Vec<CompletionItem> {
    names
        .into_iter()
        .map(|name| {
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
