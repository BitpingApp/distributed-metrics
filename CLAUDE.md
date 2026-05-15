# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Refer to README.md for project overview, supported protocols, configuration, and usage.

## Monorepo context

This directory is a **git subtree** of https://github.com/BitpingApp/distributed-metrics (remote: `distributed-metrics-public`). The monorepo is the source of truth — code is mirrored to GitHub but builds and releases live here because the workspace shares dependency pins with other monorepo crates (see root `Cargo.toml`).

The standalone Nix flake, `dist-workspace.toml`, and `.github/workflows/` were removed when this joined the monorepo — those presupposed standalone GitHub-driven builds that can't resolve the workspace deps anymore. Release flow now: monorepo GitLab CI → GoReleaser → GitHub Release on `BitpingApp/distributed-metrics`. See root `CLAUDE.md` "Sources of truth: subtrees" for the full picture.

## Build Commands

```bash
# From monorepo root:
cargo check -p distributed-metrics
cargo build -p distributed-metrics --release
cargo run -p distributed-metrics
```

## Testing

```bash
cargo test -p distributed-metrics                                          # Unit tests only
cargo test -p distributed-metrics --test remote_write -- --ignored          # Integration tests (requires Docker/Podman)
cargo test -p distributed-metrics --test remote_write -- --ignored --nocapture
```

Integration tests spin up VictoriaMetrics and Prometheus containers via testcontainers, push metrics through the real `RemoteWriteSender::push_once` code path, and query back to verify correctness. Each test suite runs against both backends.

## Releasing

Triggered manually via GitLab UI → "Run pipeline" with `DISTRIBUTED_METRICS_RELEASE_VERSION=X.Y.Z`. Before pressing go, push the subtree so the GitHub commit matches the source the binaries are built from:

```bash
just distributed-metrics-push
```

GoReleaser publishes to GitHub Releases, Homebrew tap (`BitpingApp/homebrew-tap`), AUR (`distributed-metrics-bin`), deb/rpm/apk, and ghcr.io. Auto-changelog is driven by conventional-commit scope (`feat(distributed-metrics): ...`) — see root CLAUDE.md "Commit Policy".

## Rules

- No unwraps allowed
