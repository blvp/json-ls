# json-ls

> [!WARNING]
> **Deprecated and unmaintained as of 2026-09-02. This repository is archived and read-only.**
>
> Use **[`vscode-json-language-server`](https://github.com/hrsh7th/vscode-langservers-extracted)**
> instead — the JSON server extracted from VS Code (upstream:
> [`microsoft/vscode-json-languageservice`](https://github.com/microsoft/vscode-json-languageservice)).
> It is dramatically faster and far more capable than this project: SchemaStore catalog
> matching, JSONC, formatting, folding ranges, document symbols, selection ranges, and
> schema association through client settings. Maintaining a competitor to it is not a
> reasonable use of anyone's time.
>
> **Migrating (Neovim ≥ 0.11):**
>
> ```sh
> npm i -g vscode-langservers-extracted    # or :MasonInstall json-lsp
> ```
>
> ```lua
> vim.lsp.enable("jsonls")   -- config ships with nvim-lspconfig
> ```
>
> Then drop your `vim.lsp.config["json-ls"]` block and uninstall the binary:
>
> ```sh
> brew uninstall json-ls && brew untap blvp/tap
> ```
>
> The v0.1.1 release binaries stay downloadable, but will receive no further fixes.


A JSON Language Server that detects `"$schema"` in JSON files, fetches the referenced JSON Schema,
and provides diagnostics, hover, and completion via the LSP protocol.

## Features

- **Diagnostics** — JSON Schema validation, 300 ms debounced
- **Hover** — description, type, default, enum values, examples
- **Completion** — property names + enum / type-based value snippets

## Installation

### Homebrew (macOS / Linux)

```sh
brew tap blvp/tap
brew install json-ls
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/blvp/json-ls/releases/latest):

| Platform | Asset |
|----------|-------|
| macOS ARM | `json-ls-*-aarch64-macos.tar.gz` |
| macOS Intel | `json-ls-*-x86_64-macos.tar.gz` |
| Linux ARM | `json-ls-*-aarch64-linux.tar.gz` |
| Linux x86_64 | `json-ls-*-x86_64-linux.tar.gz` |
| Windows x86_64 | `json-ls-*-x86_64-windows.zip` |

Extract and place the `json-ls` binary somewhere on your `$PATH`.

### Cargo

```sh
cargo install --git https://github.com/blvp/json-ls
```

### Build from source

```sh
make build
make install   # installs to ~/.local/bin/json-ls
```

## Neovim Setup

Requires Neovim ≥ 0.11. Add to your config:

```lua
vim.lsp.config["json-ls"] = {
  cmd = { "json-ls" },
  filetypes = { "json", "jsonc" },
  root_markers = { ".git", "package.json", ".editorconfig" },
  single_file_support = true,
  init_options = {
    schema_ttl_secs       = 28800,  -- schema cache TTL (seconds)
    schema_cache_capacity = 128,    -- max schemas in memory
  },
}
vim.lsp.enable("json-ls")
```

### With nvim-cmp / blink.cmp

Pass extended capabilities before calling `vim.lsp.enable`:

```lua
vim.lsp.config["json-ls"] = {
  cmd = { "json-ls" },
  filetypes = { "json", "jsonc" },
  root_markers = { ".git", "package.json", ".editorconfig" },
  single_file_support = true,
  capabilities = require("cmp_nvim_lsp").default_capabilities(),
  init_options = {
    schema_ttl_secs       = 28800,
    schema_cache_capacity = 128,
  },
}
vim.lsp.enable("json-ls")
```

### Other LSP clients

`json-ls` speaks standard LSP over stdio. Point any LSP client at the `json-ls` binary.

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
