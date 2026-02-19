use crate::document::DocumentStore;
use crate::position::{position_to_context, PositionContext};
use crate::schema::{SchemaCache, SchemaNode};
use std::sync::Arc;
use tower_lsp::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};
use tracing::debug;

pub async fn handle_hover(
    documents: &Arc<DocumentStore>,
    schema_cache: &Arc<SchemaCache>,
    params: HoverParams,
) -> Option<Hover> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;

    let text = documents.get_text(uri)?;
    let schema_url = documents.get_schema_url(uri)?;

    let context = position_to_context(&text, pos.line, pos.character);
    debug!("Hover context: {context:?}");

    let path = match &context {
        PositionContext::Value { path } | PositionContext::Key { path } => path.clone(),
        _ => return None,
    };

    let schema_value = schema_cache.get_or_fetch(&schema_url).await.ok()?;
    let root_node = SchemaNode::new(&schema_value, &schema_value);
    let node = root_node.navigate(&path)?;

    let info = node.hover_info();
    let markdown = info.to_markdown();

    if markdown.is_empty() {
        return None;
    }

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    })
}
