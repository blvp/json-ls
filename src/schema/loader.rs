use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, instrument};

const USER_AGENT: &str = "json-ls.nvim/0.1";
const TIMEOUT_SECS: u64 = 10;

/// Fetch a JSON schema from an HTTP(S) URL or a `file://` / bare path.
#[instrument(skip_all, fields(url = %url))]
pub async fn load_schema(url: &str) -> Result<Value> {
    if url.starts_with("http://") || url.starts_with("https://") {
        load_http(url).await
    } else {
        let path = url
            .strip_prefix("file://")
            .or_else(|| url.strip_prefix("file:"))
            .unwrap_or(url);
        load_file(path)
    }
}

fn load_file(path: &str) -> Result<Value> {
    debug!("Loading schema from file: {path}");
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read schema file: {path}"))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse schema JSON from: {path}"))
}

async fn load_http(url: &str) -> Result<Value> {
    debug!("Fetching schema over HTTP: {url}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to build HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP request failed for: {url}"))?;

    if !response.status().is_success() {
        bail!(
            "HTTP {status} fetching schema: {url}",
            status = response.status()
        );
    }

    response
        .json::<Value>()
        .await
        .with_context(|| format!("Failed to parse JSON schema from: {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_file_schema() {
        let schema_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/simple-schema.json"
        );
        let url = format!("file://{schema_path}");
        let result = load_schema(&url).await;
        assert!(
            result.is_ok(),
            "Expected schema load to succeed: {result:?}"
        );
        let schema = result.unwrap();
        assert!(
            schema.get("type").is_some()
                || schema.get("properties").is_some()
                || schema.get("$schema").is_some()
        );
    }
}
