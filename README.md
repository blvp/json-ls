# json-ls

A JSON Language Server that detects `"$schema"` in JSON files, fetches the referenced JSON Schema,
and provides diagnostics, hover, and completion via the LSP protocol.

## Installation

```sh
cargo install --git https://github.com/blvp/json-ls
```

Or build from source:

```sh
make build
make install   # installs to ~/.local/bin/json-ls
```

## Usage

Intended to be used with the [json-ls.nvim](https://github.com/blvp/json-ls.nvim) Neovim plugin,
or any LSP client that supports stdio JSON-RPC transport.

## Features

- **Diagnostics** — JSON Schema validation, 300 ms debounced
- **Hover** — description, type, default, enum values, examples
- **Completion** — property names + enum / type-based value snippets

## Configuration (`initializationOptions`)

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `schema_ttl_secs` | u64 | 28800 | Schema cache TTL in seconds |
| `schema_cache_capacity` | u64 | 128 | Max schemas held in memory |

## Development

```sh
make test       # cargo test
make lint       # cargo clippy
make fmt-check  # cargo fmt --check
make ci         # all checks + build
```

## License

MIT
