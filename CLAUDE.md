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

## Testing

```bash
cargo test                                            # Unit tests only
cargo test --test remote_write -- --ignored           # Integration tests (requires Docker/Podman)
cargo test --test remote_write -- --ignored --nocapture # Integration tests with output
```

Integration tests spin up VictoriaMetrics and Prometheus containers via testcontainers, push metrics through the real `RemoteWriteSender::push_once` code path, and query back to verify correctness. Each test suite runs against both backends.

## Flake Maintenance

When `Cargo.toml` changes (version bump or dependency changes):
- Update `version` in `flake.nix` to match `Cargo.toml`
- Set `cargoHash` to `""` and rebuild to get the new hash from the error output, then update it

## Rules

- No unwraps allowed
