use tower_lsp::{LspService, Server};
use tracing_subscriber::{fmt, EnvFilter};

mod backend;
mod completion;
mod config;
mod diagnostics;
mod document;
mod hover;
mod position;
mod schema;

use backend::Backend;

#[tokio::main]
async fn main() {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
