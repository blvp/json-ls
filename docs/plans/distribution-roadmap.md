# json-ls Distribution Roadmap

Current state: v0.1.0 pre-release, binary only, installed manually via `cargo install` or `make install`.

---

## Track 1 — GitHub Actions CI/CD (prerequisite for everything else)

### 1a. CI workflow (`ci.yml`)
Trigger: push + pull_request on `main`.

Steps:
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test` (unit)
- `cargo build` + `cargo test --test lsp_harness` (integration)

Runs on: `ubuntu-latest`, `macos-latest`.

### 1b. Release workflow (`release.yml`)
Trigger: push of tag matching `v[0-9]+.*`.

Steps:
1. Parse version from tag, verify it matches `Cargo.toml`.
2. Cross-compile for all targets (use `cross` tool or GitHub-hosted runners):
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
   - `x86_64-pc-windows-msvc`
3. Package each binary:
   - Unix: `json-ls-{version}-{target}.tar.gz` containing `json-ls`
   - Windows: `json-ls-{version}-{target}.zip` containing `json-ls.exe`
4. Generate SHA256 checksums (`json-ls-{version}-checksums.txt`).
5. Create GitHub Release (use `softprops/action-gh-release`) — attach all archives + checksums.
6. Update the Homebrew formula in the tap repo (see Track 3).

Asset naming convention (required for Mason):
```
json-ls-{version}-x86_64-linux.tar.gz
json-ls-{version}-aarch64-linux.tar.gz
json-ls-{version}-x86_64-macos.tar.gz
json-ls-{version}-aarch64-macos.tar.gz
json-ls-{version}-x86_64-windows.zip
```

---

## Track 2 — Semver + Release Procedure

### Version policy
- `MAJOR`: breaking LSP protocol changes or removed config keys
- `MINOR`: new LSP capabilities (e.g., go-to-definition, code actions, formatting)
- `PATCH`: bug fixes, performance improvements, schema navigator improvements

### Release checklist
1. Update version in `Cargo.toml` (and `Cargo.lock` via `cargo build`).
2. Update `CHANGELOG.md` (keep a changelog format).
3. Commit: `chore: release v{version}`.
4. Tag: `git tag -s v{version} -m "v{version}"` (signed tag preferred).
5. Push tag: `git push origin v{version}`.
6. GitHub Actions release workflow fires automatically.

### First release: v0.1.0
- Tag the current `main` HEAD as `v0.1.0`.
- Marks the fix/hover-key-path work as part of the official release.

---

## Track 3 — Homebrew Tap

Create repo: `github.com/blvp/homebrew-tap`

Formula file: `Formula/json-ls.rb`

```ruby
class JsonLs < Formula
  desc "JSON LSP server with automatic $schema-driven validation, hover, and completion"
  homepage "https://github.com/blvp/json-ls"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/blvp/json-ls/releases/download/v#{version}/json-ls-#{version}-aarch64-macos.tar.gz"
      sha256 "<sha256>"
    end
    on_intel do
      url "https://github.com/blvp/json-ls/releases/download/v#{version}/json-ls-#{version}-x86_64-macos.tar.gz"
      sha256 "<sha256>"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/blvp/json-ls/releases/download/v#{version}/json-ls-#{version}-aarch64-linux.tar.gz"
      sha256 "<sha256>"
    end
    on_intel do
      url "https://github.com/blvp/json-ls/releases/download/v#{version}/json-ls-#{version}-x86_64-linux.tar.gz"
      sha256 "<sha256>"
    end
  end

  def install
    bin.install "json-ls"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/json-ls --version 2>&1", 1)
  end
end
```

Note: `--version` flag is not yet implemented in the binary — add it before publishing.

**Installation for users:**
```sh
brew tap blvp/tap
brew install json-ls
```

**Automation:** The release workflow should auto-update the formula by bumping `version` and `sha256` values via a commit to the tap repo.

### Path to homebrew-core (optional, later)
Once the tool has traction, submit to `homebrew/homebrew-core`. Requires:
- Stable release history
- Active maintenance signal
- Passes `brew audit --strict`

---

## Track 4 — Mason Registry

Mason registry: `github.com/mason-org/mason-registry`

File to add: `packages/json-ls/package.yaml`

```yaml
name: json-ls
description: JSON LSP server with automatic $schema-driven validation, hover, and completion
homepage: https://github.com/blvp/json-ls
licenses:
  - MIT
languages:
  - JSON
categories:
  - LSP

source:
  id: pkg:github/blvp/json-ls@v0.1.0
  asset:
    - target: darwin_arm64
      file: json-ls-{{version}}-aarch64-macos.tar.gz
    - target: darwin_x64
      file: json-ls-{{version}}-x86_64-macos.tar.gz
    - target: linux_arm64
      file: json-ls-{{version}}-aarch64-linux.tar.gz
    - target: linux_x64
      file: json-ls-{{version}}-x86_64-linux.tar.gz
    - target: win_x64
      file: json-ls-{{version}}-x86_64-windows.zip

bin:
  json-ls: json-ls        # json-ls.exe on Windows (Mason handles extension)
```

**Prerequisites before submitting PR:**
- At least one stable tagged release with all platform binaries
- Binary passes `json-ls --version` (Mason validates this)
- All asset URLs resolve and checksums match
- Follow mason-registry PR template (automated checks run on CI)

---

## Dependency Order

```
1. Add --version flag to binary
2. Track 1a: CI workflow
3. Track 2: First v0.1.0 tag + release (manual)
4. Track 1b: Release workflow (automates future releases)
5. Track 3: Homebrew tap (create repo + initial formula)
6. Track 4: Mason registry PR (after ≥1 stable release exists)
7. Track 3b: homebrew-core (optional, once established)
```

---

## Small prerequisites not tracked above

- `--version` flag: Mason and Homebrew test blocks require it. Add to `main.rs`:
  ```rust
  if std::env::args().any(|a| a == "--version") {
      println!("{}", env!("CARGO_PKG_VERSION"));
      return;
  }
  ```
- `CHANGELOG.md`: Start before v0.1.0 tag.
- Signed tags: Recommended for release integrity (`git tag -s`).
