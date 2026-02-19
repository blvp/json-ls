use crate::completion::handle_completion;
use crate::config::ServerConfig;
use crate::diagnostics::validate_document;
use crate::document::DocumentStore;
use crate::hover::handle_hover;
use crate::schema::SchemaCache;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, info};

const DEBOUNCE_MS: u64 = 300;

pub struct Backend {
    client: Client,
    documents: Arc<DocumentStore>,
    schema_cache: Arc<SchemaCache>,
    pending_diagnostics: Arc<DashMap<Url, JoinHandle<()>>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let config = ServerConfig::default();
        let schema_cache = Arc::new(SchemaCache::new(&config));

        Self {
            client,
            documents: Arc::new(DocumentStore::new()),
            schema_cache,
            pending_diagnostics: Arc::new(DashMap::new()),
        }
    }

    fn schedule_diagnostics(&self, uri: Url) {
        // Abort any in-flight diagnostic task for this document
        if let Some((_, handle)) = self.pending_diagnostics.remove(&uri) {
            handle.abort();
        }

        let client = self.client.clone();
        let documents = self.documents.clone();
        let schema_cache = self.schema_cache.clone();
        let pending = self.pending_diagnostics.clone();
        let task_uri = uri.clone();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(DEBOUNCE_MS)).await;

            let diagnostics = validate_document(&task_uri, &documents, &schema_cache)
                .await
                .unwrap_or_default();

            client
                .publish_diagnostics(task_uri.clone(), diagnostics, None)
                .await;

            pending.remove(&task_uri);
        });

        self.pending_diagnostics.insert(uri, handle);
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Parse server config from initializationOptions
        let config = params
            .initialization_options
            .map(ServerConfig::from_value)
            .unwrap_or_default();

        info!("json-ls initializing with config: {config:?}");

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "json-ls".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["\"".into(), ":".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("json-ls server ready");
        self.client
            .log_message(MessageType::INFO, "json-ls initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        // Abort all pending diagnostic tasks
        for entry in self.pending_diagnostics.iter() {
            entry.value().abort();
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let text = params.text_document.text;

        debug!("did_open: {uri}");
        self.documents.open(uri.clone(), version, text);

        // Prefetch the schema eagerly so it is cached before the first completion request.
        // This runs in its own task so it is never cancelled by did_change debouncing.
        if let Some(schema_url) = self.documents.get_schema_url(&uri) {
            let cache = self.schema_cache.clone();
            tokio::spawn(async move {
                let _ = cache.get_or_fetch(&schema_url).await;
            });
        }

        self.schedule_diagnostics(uri);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;

        debug!("did_change: {uri} v{version}");

        if let Err(e) = self.documents.update(&uri, version, params.content_changes) {
            self.client
                .log_message(
                    MessageType::ERROR,
                    format!("Failed to update document: {e}"),
                )
                .await;
            return;
        }

        self.schedule_diagnostics(uri);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = &params.text_document.uri;
        debug!("did_close: {uri}");

        // Clear pending diagnostics
        if let Some((_, handle)) = self.pending_diagnostics.remove(uri) {
            handle.abort();
        }

        self.documents.close(uri);

        // Clear diagnostics for closed file
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        Ok(handle_hover(&self.documents, &self.schema_cache, params).await)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        Ok(handle_completion(&self.documents, &self.schema_cache, params).await)
    }
}
