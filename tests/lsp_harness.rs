//! Integration tests: spawn json-ls as a child process and drive it via
//! raw LSP JSON-RPC over stdin/stdout.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

const BINARY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/json-ls");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const DIAG_TIMEOUT_SECS: u64 = 6; // 300 ms debounce + network + headroom

fn schema_file_url() -> String {
    format!("file://{FIXTURES}/simple-schema.json")
}

struct LspClient {
    stdin: Mutex<tokio::process::ChildStdin>,
    next_id: Arc<AtomicI64>,
    pending_tx: Arc<Mutex<std::collections::HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    notifications: Arc<Mutex<VecDeque<Value>>>,
    _child: Child,
}

impl LspClient {
    async fn spawn() -> Self {
        let mut child = Command::new(BINARY)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn json-ls. Run `cargo build` first.");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let pending_tx: Arc<
            Mutex<std::collections::HashMap<i64, tokio::sync::oneshot::Sender<Value>>>,
        > = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let notifications: Arc<Mutex<VecDeque<Value>>> = Arc::new(Mutex::new(VecDeque::new()));

        // Background reader task
        let pending_tx_bg = pending_tx.clone();
        let notifications_bg = notifications.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                // Read headers
                let mut content_length: Option<usize> = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return; // EOF
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(val) = line.strip_prefix("Content-Length: ") {
                        content_length = val.trim().parse().ok();
                    }
                }
                let len = match content_length {
                    Some(l) => l,
                    None => continue,
                };
                // Read body
                let mut buf = vec![0u8; len];
                if reader.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let msg: Value = match serde_json::from_slice(&buf) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // Route: response (has id, no method) vs notification (has method, no id)
                let id = msg.get("id").and_then(|v| v.as_i64());
                let has_method = msg.get("method").is_some();

                if let Some(id) = id {
                    if !has_method {
                        // Response to a request
                        let sender = pending_tx_bg.lock().await.remove(&id);
                        if let Some(tx) = sender {
                            let _ = tx.send(msg);
                        }
                        continue;
                    }
                }
                // Notification or server-initiated request
                notifications_bg.lock().await.push_back(msg);
            }
        });

        Self {
            stdin: Mutex::new(stdin),
            next_id: Arc::new(AtomicI64::new(1)),
            pending_tx,
            notifications,
            _child: child,
        }
    }

    async fn send_request(&self, method: &str, params: Option<Value>) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            msg["params"] = p;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_tx.lock().await.insert(id, tx);
        self.write_message(&msg).await;
        timeout(Duration::from_secs(10), rx)
            .await
            .expect("Request timed out")
            .expect("Response channel dropped")
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) {
        let mut msg = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_message(&msg).await;
    }

    async fn write_message(&self, msg: &Value) {
        let body = serde_json::to_string(msg).unwrap();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(frame.as_bytes()).await.unwrap();
    }

    async fn wait_for_notification(&self, method: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(DIAG_TIMEOUT_SECS);
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Timeout waiting for notification: {method}"
            );
            let found = {
                let mut queue = self.notifications.lock().await;
                let pos = queue
                    .iter()
                    .position(|n| n["method"].as_str() == Some(method));
                pos.map(|i| {
                    let mut v: Vec<Value> = queue.drain(..).collect();
                    let found = v.remove(i);
                    *queue = v.into();
                    found
                })
            };
            if let Some(notif) = found {
                return notif;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn initialize(&self) -> Value {
        let resp = self
            .send_request(
                "initialize",
                Some(json!({
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {},
                    "initializationOptions": {
                        "schema_ttl_secs": 60,
                        "schema_cache_capacity": 16
                    }
                })),
            )
            .await;
        self.send_notification("initialized", Some(json!({}))).await;
        resp
    }

    /// Open a document. `schema_url` is injected as the `$schema` value.
    /// Use `None` to omit the `$schema` key entirely.
    async fn open_document(&self, uri: &str, schema_url: Option<&str>, body_fields: &str) {
        let text = match schema_url {
            Some(url) => format!("{{\n  \"$schema\": \"{url}\",\n  {body_fields}\n}}"),
            None => format!("{{\n  {body_fields}\n}}"),
        };
        self.send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "json",
                    "version": 1,
                    "text": text,
                }
            })),
        )
        .await;
    }

    async fn shutdown(&self) {
        self.send_request("shutdown", None).await;
        self.send_notification("exit", None).await;
    }
} // end impl LspClient

#[tokio::test]
async fn test_initialize() {
    let client = LspClient::spawn().await;
    let resp = client.initialize().await;

    let caps = &resp["result"]["capabilities"];
    assert!(
        caps["hoverProvider"].as_bool().unwrap_or(false),
        "Expected hoverProvider=true, got: {caps}"
    );
    assert!(
        caps["completionProvider"].is_object(),
        "Expected completionProvider object, got: {caps}"
    );
    assert!(
        caps["textDocumentSync"].is_object() || caps["textDocumentSync"].is_number(),
        "Expected textDocumentSync, got: {caps}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn test_lifecycle_shutdown() {
    let client = LspClient::spawn().await;
    client.initialize().await;
    let shutdown_resp = client.send_request("shutdown", None).await;
    // Shutdown must return null result, no error
    assert!(
        shutdown_resp["error"].is_null(),
        "Shutdown returned error: {shutdown_resp}"
    );
    assert!(
        shutdown_resp["result"].is_null(),
        "Shutdown result should be null, got: {shutdown_resp}"
    );
    client.send_notification("exit", None).await;
}

#[tokio::test]
async fn test_diagnostics_valid_document() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    let schema_url = schema_file_url();
    client
        .open_document(
            "file:///tmp/valid.json",
            Some(&schema_url),
            r#""name": "hello", "count": 42, "enabled": true"#,
        )
        .await;

    let notif = client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = &notif["params"]["diagnostics"];
    assert!(
        diagnostics
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "Expected no diagnostics for valid document, got: {diagnostics}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn test_diagnostics_invalid_document() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    let schema_url = schema_file_url();
    // "name" is required but missing; "count" is wrong type
    client
        .open_document(
            "file:///tmp/invalid.json",
            Some(&schema_url),
            r#""count": "not-a-number""#,
        )
        .await;

    let notif = client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = notif["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics should be an array");
    assert!(
        diagnostics.len() >= 1,
        "Expected at least 1 diagnostic (missing required 'name' or wrong type for 'count'), got: {diagnostics:?}"
    );
    // All diagnostics should be from json-ls
    for d in diagnostics {
        assert_eq!(
            d["source"].as_str(),
            Some("json-ls"),
            "Unexpected source: {d}"
        );
    }
    client.shutdown().await;
}

#[tokio::test]
async fn test_hover_key() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    let schema_url = schema_file_url();
    // Build document with each field on its own line for accurate position scanning
    // Line 0: {
    // Line 1:   "$schema": "...",
    // Line 2:   "name": "hello",
    // Line 3:   "count": 42
    // Line 4: }
    // Hover at line 2, character 11 — inside "hello" value of "name" key
    // Line 2: `  "name": "hello",`
    //          0123456789012345
    // Character 11 is inside the value string "hello"
    let text = format!(
        "{{\n  \"$schema\": \"{schema_url}\",\n  \"name\": \"hello\",\n  \"count\": 42\n}}"
    );
    client
        .send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": "file:///tmp/hover.json",
                    "languageId": "json",
                    "version": 1,
                    "text": text,
                }
            })),
        )
        .await;

    // Wait for diagnostics to confirm server processed the document
    client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;

    let resp = client
        .send_request(
            "textDocument/hover",
            Some(json!({
                "textDocument": { "uri": "file:///tmp/hover.json" },
                "position": { "line": 2, "character": 11 }
            })),
        )
        .await;

    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "Expected a hover result, got null. resp: {resp}"
    );
    let contents = result["contents"]["value"].as_str().unwrap_or("");
    assert!(
        contents.contains("name") || contents.contains("The name") || contents.contains("string"),
        "Expected hover to mention 'name', its description, or type 'string', got: {contents:?}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn test_completion_property_names() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    let schema_url = schema_file_url();
    // Open a document with an incomplete key so cursor is at key-start position
    // Line 0: {
    // Line 1:   "$schema": "...",
    // Line 2:   ""
    // Trigger completion at line 2, character 3 (inside the opening quote of a key)
    let text = format!("{{\n  \"$schema\": \"{schema_url}\",\n  \"\"\n}}");
    client
        .send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": "file:///tmp/completion.json",
                    "languageId": "json",
                    "version": 1,
                    "text": text,
                }
            })),
        )
        .await;

    // Wait for the server to process the document
    client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;

    let resp = client
        .send_request(
            "textDocument/completion",
            Some(json!({
                "textDocument": { "uri": "file:///tmp/completion.json" },
                "position": { "line": 2, "character": 3 }
            })),
        )
        .await;

    let items = resp["result"]
        .as_array()
        .expect("completion result should be an array");
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();

    assert!(
        labels.contains(&"name"),
        "Expected 'name' in completions, got: {labels:?}"
    );
    assert!(
        labels.contains(&"count"),
        "Expected 'count' in completions, got: {labels:?}"
    );
    assert!(
        labels.contains(&"enabled"),
        "Expected 'enabled' in completions, got: {labels:?}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn test_no_schema_key_produces_no_diagnostics() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    // Document with no "$schema" key
    client
        .open_document(
            "file:///tmp/no-schema.json",
            None, // no $schema
            r#""name": "hello", "count": 42"#,
        )
        .await;

    let notif = client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = &notif["params"]["diagnostics"];
    assert!(
        diagnostics
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "Expected no diagnostics when $schema is absent, got: {diagnostics}"
    );

    // Also verify hover returns null (no schema to look up)
    let resp = client
        .send_request(
            "textDocument/hover",
            Some(json!({
                "textDocument": { "uri": "file:///tmp/no-schema.json" },
                "position": { "line": 1, "character": 3 }
            })),
        )
        .await;
    assert!(
        resp["result"].is_null(),
        "Expected null hover result without $schema, got: {resp}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn test_malformed_json_produces_syntax_diagnostic() {
    let client = LspClient::spawn().await;
    client.initialize().await;

    let schema_url = schema_file_url();
    // Truncated JSON — serde_json will fail to parse
    let broken_text = format!("{{\"$schema\": \"{schema_url}\", \"name\": \"hello\", \"count\":");
    client
        .send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": "file:///tmp/malformed.json",
                    "languageId": "json",
                    "version": 1,
                    "text": broken_text,
                }
            })),
        )
        .await;

    let notif = client
        .wait_for_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = notif["params"]["diagnostics"]
        .as_array()
        .expect("Expected diagnostics array");
    assert_eq!(
        diagnostics.len(),
        1,
        "Expected exactly 1 syntax error diagnostic, got: {diagnostics:?}"
    );
    assert_eq!(
        diagnostics[0]["code"].as_str(),
        Some("json-syntax"),
        "Expected code='json-syntax', got: {:?}",
        diagnostics[0]["code"]
    );

    client.shutdown().await;
}
