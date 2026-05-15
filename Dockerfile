# Dockerfile used by GoReleaser (see .goreleaser.yaml).
# GoReleaser invokes `docker buildx build --platform=linux/<arch>` inside a
# directory where the pre-built binary already lives. We just copy it into a
# minimal runtime image.
#
# Do NOT build distributed-metrics from source here — the binary in this
# Dockerfile's context was cross-compiled by the parent GitLab CI job.
# Building from source would require the full monorepo workspace, which
# isn't present in GoReleaser's per-arch build context.

FROM debian:trixie-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY distributed-metrics /usr/local/bin/distributed-metrics

WORKDIR /app

EXPOSE 3000

CMD ["/usr/local/bin/distributed-metrics"]
