# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Refer to README.md for project overview, supported protocols, configuration, and usage.

## Build Commands

```bash
nix build            # Build with Nix
nix develop          # Dev shell (rustc, cargo, cargo-audit, cargo-watch)
nix run              # Run directly
cargo watch -x run   # Hot-reload during development (inside dev shell)
```

## Flake Maintenance

When `Cargo.toml` changes (version bump or dependency changes):
- Update `version` in `flake.nix` to match `Cargo.toml`
- Set `cargoHash` to `""` and rebuild to get the new hash from the error output, then update it
