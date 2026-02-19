use crate::config::ServerConfig;
use crate::schema::loader::load_schema;
use anyhow::{anyhow, Result};
use dashmap::DashMap;
use moka::future::Cache;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const ERROR_RETRY_SECS: u64 = 60;

pub struct SchemaCache {
    inner: Cache<String, Arc<Value>>,
    errors: DashMap<String, Instant>,
}

impl SchemaCache {
    pub fn new(config: &ServerConfig) -> Self {
        let inner = Cache::builder()
            .max_capacity(config.schema_cache_capacity)
            .time_to_live(Duration::from_secs(config.schema_ttl_secs))
            .build();

        Self {
            inner,
            errors: DashMap::new(),
        }
    }

    /// Return a cached schema, fetching it if not present.
    ///
    /// Failed fetches are NOT cached in moka; instead we store an error timestamp
    /// and refuse to retry for `ERROR_RETRY_SECS` seconds.
    pub async fn get_or_fetch(&self, url: &str) -> Result<Arc<Value>> {
        // Check error cooldown
        if let Some(failed_at) = self.errors.get(url) {
            if failed_at.elapsed() < Duration::from_secs(ERROR_RETRY_SECS) {
                debug!("Schema fetch on cooldown: {url}");
                return Err(anyhow!("Schema fetch on cooldown for: {url}"));
            }
            // Cooldown expired — allow retry
            drop(failed_at);
            self.errors.remove(url);
        }

        let url_owned = url.to_owned();
        let errors = self.errors.clone();

        // get_with coalesces concurrent fetches for the same URL
        let result = self
            .inner
            .try_get_with(url_owned.clone(), async move {
                match load_schema(&url_owned).await {
                    Ok(schema) => {
                        debug!("Schema loaded and cached: {url_owned}");
                        Ok(Arc::new(schema))
                    }
                    Err(e) => {
                        warn!("Failed to fetch schema {url_owned}: {e}");
                        errors.insert(url_owned, Instant::now());
                        Err(e)
                    }
                }
            })
            .await;

        result.map_err(|e| anyhow!("{e}"))
    }

    // TODO: wire up to a `workspace/executeCommand` handler so editors can force-refresh
    // a specific schema URL without restarting the server (e.g. after editing a local schema).
    #[allow(dead_code)]
    pub fn invalidate(&self, url: &str) {
        let cache = self.inner.clone();
        let url_owned = url.to_owned();
        self.errors.remove(&url_owned);
        tokio::spawn(async move {
            cache.invalidate(&url_owned).await;
        });
    }
}
