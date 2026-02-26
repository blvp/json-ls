# Changelog

All notable changes to this project will be documented in this file.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

## [0.1.0] - 2026-02-26

### Added
- Diagnostics: JSON Schema validation via `jsonschema 0.42`, debounced 300 ms
- Hover: description, type, default, enum values, examples from schema
- Completion: property names + enum / type-based value snippets
- Schema loader: HTTP and `file://` URLs, 10 s timeout, Moka TTL cache
- Schema navigator: `$ref`, `allOf`, `anyOf`, `oneOf`, `items`, `prefixItems`, cycle detection
- `--version` / `-V` flag

### Fixed
- Hover on a key string now returns docs for that field, not the parent object
  (`PositionContext::Key { path }` now stores the full path to the key)
