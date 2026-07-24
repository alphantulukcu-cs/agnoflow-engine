# syntax=docker/dockerfile:1
# --- Builder: Rust workspace'i derle (sqlx compile-time makro YOK → DB gerekmez) ---
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
# Yalnız server binary'sini derle (wf-server). rustls kullanılıyor → OpenSSL gerekmez.
RUN cargo build --release -p wf-server

# --- Runtime: küçük imaj, non-root ---
FROM debian:bookworm-slim
# ca-certificates: autoexec REST/S3 çağrıları için giden TLS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# uid 1000 = deployment securityContext ile hizalı; PVC (/var/lib/agnoflow) fsGroup 1000 ile yazılabilir.
RUN useradd -u 1000 -m -s /usr/sbin/nologin app \
    && mkdir -p /var/lib/agnoflow \
    && chown -R app:app /var/lib/agnoflow
COPY --from=builder /app/target/release/wf-server /usr/local/bin/wf-server
USER 1000
ENV PORT=3000
EXPOSE 3000
CMD ["wf-server"]
