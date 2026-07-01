FROM rustlang/rust:nightly AS builder

WORKDIR /app

COPY . .

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    curl \
    git && \
    rm -rf /var/lib/apt/lists/*

RUN curl -L https://foundry.paradigm.xyz | bash && \
    /root/.foundry/bin/foundryup

RUN cargo build --release

FROM rustlang/rust:nightly

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl-dev && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /root/.foundry/bin/anvil /usr/local/bin/anvil
COPY --from=builder /app/target/release/andromeda-galaxy /usr/local/bin/server

WORKDIR /app

COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

EXPOSE 8080

VOLUME ["/app/data"]

ENV SQLITE_PATH=/app/data/andromeda.db
ENV RUST_LOG=info

HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

CMD ["/app/entrypoint.sh"]
