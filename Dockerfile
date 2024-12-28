FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Download and run the installer
RUN curl --proto '=https' --tlsv1.2 -LsSf https://github.com/BitpingApp/distributed-metrics/releases/latest/download/distributed-metrics-installer.sh -o installer.sh

RUN chmod +x installer.sh

RUN ./installer.sh

WORKDIR /app

# Move the binary to a location in PATH
RUN mv ~/.cargo/bin/distributed-metrics /app/distributed-metrics

EXPOSE 3000
CMD ["/app/distributed-metrics"]

