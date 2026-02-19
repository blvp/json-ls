use serde::Deserialize;
use std::path::PathBuf;

const DEFAULT_SCHEMA_TTL_SECS: u64 = 28800; // 8 hours
const DEFAULT_SCHEMA_CACHE_CAPACITY: u64 = 128;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_ttl")]
    pub schema_ttl_secs: u64,

    // TODO: implement persistent disk caching — serialize fetched schemas to cache_dir so
    // they survive server restarts without a network round-trip.
    #[allow(dead_code)]
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,

    #[serde(default = "default_cache_capacity")]
    pub schema_cache_capacity: u64,
}

fn default_ttl() -> u64 {
    DEFAULT_SCHEMA_TTL_SECS
}

fn default_cache_capacity() -> u64 {
    DEFAULT_SCHEMA_CACHE_CAPACITY
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            schema_ttl_secs: DEFAULT_SCHEMA_TTL_SECS,
            cache_dir: None,
            schema_cache_capacity: DEFAULT_SCHEMA_CACHE_CAPACITY,
        }
    }
}

impl ServerConfig {
    pub fn from_value(value: serde_json::Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }
}
