FROM gcr.io/distroless/cc-debian12:nonroot
COPY --chmod=755 distributed-metrics /usr/local/bin/distributed-metrics
WORKDIR /app
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/distributed-metrics"]
