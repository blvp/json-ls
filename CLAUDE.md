# json-ls — CLAUDE.md

## Project Status

**v0.1.0 — pre-release, ready to tag.**
Rust LSP server binary only. No Lua plugin — users wire it up via Neovim's native `vim.lsp.config`.
Core features working: diagnostics, hover, completion.
See TODO markers in source for planned follow-on work.

### Distribution To-Do (see `docs/plans/distribution-roadmap.md` for full detail)

| # | Task | Status |
|---|------|--------|
| 1 | `--version` / `-V` flag | ✅ done |
| 2 | `CHANGELOG.md` | ✅ done |
| 3 | GitHub Actions CI workflow (fmt + clippy + test, ubuntu + macos) | ✅ done |
| 4 | GitHub Actions release workflow (5-target cross-compile + GitHub Release) | ✅ done |
| 5 | Tag `v0.1.0` and push — triggers release workflow | ✅ done |
| 6 | Create `blvp/homebrew-tap` repo + `Formula/json-ls.rb` | ✅ done |
| 7 | Submit PR to `mason-org/mason-registry` | ⬜ |
| 8 | (later) Submit to `homebrew/homebrew-core` | ⬜ |
| 8 | (later) Submit to `homebrew/homebrew-core` | `brew install json-ls` without tap |

---

## Project Overview

A Rust LSP server binary (`json-ls`). The server detects `"$schema"` in JSON files, fetches the
referenced JSON Schema, and provides:

- **Diagnostics** — jsonschema validation, 300 ms debounced
- **Hover** — description, type, default, enum values, examples
- **Completion** — property names + enum / type-based value snippets

See `README.md` for user-facing documentation.

---

## Repository Layout

```
Cargo.toml               Rust workspace (single binary: json-ls)
src/
  main.rs                Tokio entry point; stdio LSP transport
  backend.rs             LanguageServer trait — dispatches all LSP methods
  config.rs              ServerConfig parsed from initializationOptions
  document.rs            DocumentStore: DashMap<Url, DocumentState> + ropey rope
  position.rs  ★         Hand-rolled byte scanner → PositionContext + JSON path
  hover.rs               hover() — delegates to schema/navigator + position
  completion.rs          completion() — property names + enum/type snippets
  diagnostics.rs         jsonschema validation → LSP Diagnostic list (debounced)
  schema/
    mod.rs               Re-exports SchemaCache, SchemaNode
    loader.rs            HTTP + file:// schema fetcher (reqwest, 10 s timeout)
    cache.rs             Moka async TTL cache + 60 s error cooldown DashMap
    navigator.rs ★       JSON Schema graph traversal: $ref, allOf/anyOf/oneOf, cycles
tests/
  fixtures/              simple-schema.json, valid-instance.json, invalid-instance.json,
                         malformed.json, no-schema.json
  lsp_harness.rs         Rust integration test harness
docs/plans/              Architecture / planning docs
```

---

## Architecture

```
Neovim (client)
    │  stdio JSON-RPC
    ▼
backend.rs  (tower-lsp LanguageServer trait)
    ├── did_open / did_change / did_close  →  document.rs  (DocumentStore)
    │                                         └── extract_schema_url()
    ├── hover / completion                 →  position.rs  (byte scanner)
    │                                         └── schema/navigator.rs  (graph walk)
    └── diagnostics (debounced 300 ms)    →  schema/cache.rs  (Moka TTL)
                                              └── schema/loader.rs  (reqwest)
                                              └── jsonschema::validator_for()
```

**Critical paths:**

- `position.rs` — UTF-16 LSP Position → byte offset → recursive-descent scan →
  `PositionContext { Key | Value | KeyStart | ValueStart | Unknown }`.
  This is the hardest module; touch carefully.

- `schema/navigator.rs` — `SchemaNode::navigate(path)` walks `properties`,
  `$ref` (JSON Pointer fragments), `allOf/anyOf/oneOf`, `items`, `prefixItems`.
  Cycle detection via `HashSet<*const Value>`.

---

## Build & Common Commands

```sh
make build        # cargo build --release
make build-debug  # cargo build (debug binary — required for integration tests)
make install      # installs target/release/json-ls → ~/.local/bin/json-ls
make test         # cargo test (unit tests)
make lint         # cargo clippy -- -D warnings
make fmt-check    # cargo fmt --check
make ci           # fmt-check + lint + test + build
```

Single test:
```sh
# Unit tests
cargo test position::tests::test_cursor_in_nested_value
cargo test schema::navigator::tests

# Rust LSP harness (integration)
cargo build && cargo test --test lsp_harness -- --nocapture
cargo test --test lsp_harness test_hover_key -- --nocapture
```

---

## Neovim Setup (for manual testing)

Wire up via native API (Neovim ≥ 0.11):

```lua
vim.lsp.config["json-ls"] = {
  name       = "json-ls",
  cmd        = { "/path/to/target/release/json-ls" },
  filetypes  = { "json", "jsonc" },
  root_markers = { ".git", "package.json" },
  single_file_support = true,
  init_options = {
    schema_ttl_secs        = 28800,
    schema_cache_capacity  = 128,
  },
}
vim.lsp.enable("json-ls")
```

Debug tracing: `RUST_LOG=json_ls=debug nvim myfile.json`

---

## Configuration Reference (initializationOptions)

| Key | Type | Default | Notes |
|---|---|---|---|
| `schema_ttl_secs` | u64 | 28800 | Schema cache TTL in seconds |
| `schema_cache_capacity` | u64 | 128 | Max schemas held in memory |
| `cache_dir` | string\|null | null | **TODO**: disk persistence not implemented |

---

## Known TODOs

Search `// TODO:` in source for all markers:

| File | Item | Notes |
|---|---|---|
| `config.rs` | `cache_dir` disk caching | Persist schemas across restarts |
| `document.rs` | `get_rope()` | Expose for future `textDocument/formatting` |
| `position.rs` | `path()` | Expose for code actions / go-to-definition |
| `schema/cache.rs` | `invalidate()` | Wire to `workspace/executeCommand` |

---

## Implementation Notes

- `serde_json` discards byte offsets after parsing → position scanning is hand-rolled in `position.rs`.
- `jsonschema 0.42` API: `validator_for(&schema)?`, `validator.iter_errors(&instance)`, `error.instance_path()`.
- `tower-lsp 0.20`: `LspService::new(Backend::new)`, all handlers take `&self` (Arc-wrapped internally).
- `moka::future::Cache::invalidate()` is async — must be `.await`ed inside a spawned task from sync context.
- `ropey` char indices ≠ UTF-16 code units — `lsp_pos_to_char_idx()` in `document.rs` handles the conversion.
- Debounce: `pending_diagnostics: DashMap<Url, JoinHandle<()>>` — abort + respawn on each `did_change`.
