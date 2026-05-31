ARG VERSION
ARG TARGETARCH

FROM alpine:3.20 AS download
ARG VERSION
ARG TARGETARCH
RUN apk add --no-cache curl tar xz ca-certificates && \
    curl -fsSL --retry 5 --retry-delay 5 -o /tmp/dm.tar.xz \
      "https://github.com/BitpingApp/distributed-metrics/releases/download/v${VERSION}/distributed-metrics-${VERSION}-linux-${TARGETARCH}.tar.xz" && \
    mkdir -p /tmp/extract && \
    tar -xJf /tmp/dm.tar.xz -C /tmp/extract && \
    mv /tmp/extract/distributed-metrics-${VERSION}-linux-${TARGETARCH}/distributed-metrics /tmp/distributed-metrics

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=download --chmod=755 /tmp/distributed-metrics /usr/local/bin/distributed-metrics
WORKDIR /app
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/distributed-metrics"]
