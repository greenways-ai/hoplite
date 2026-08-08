ARG NGINX_VERSION=1.30.4

FROM rust:1.85-bookworm AS builder
ARG NGINX_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl libpcre2-dev libssl-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY hoplite hoplite
COPY hara hara
RUN make -C hoplite/core embedded-cli server-cli
RUN cp -R hoplite/core/examples /app \
    && hoplite/core/target/release/hoplite serve build --mode prod /app

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl libpcre2-8-0 libssl3 zlib1g \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --system hoplite && useradd --system --gid hoplite --home /var/lib/hoplite hoplite \
    && mkdir -p /app /var/lib/hoplite/cache \
    && chown -R hoplite:hoplite /app /var/lib/hoplite
COPY --from=builder /src/hoplite/core/target/release/hoplite-server /usr/local/bin/hoplite-server
COPY --from=builder --chown=hoplite:hoplite /app /app
USER hoplite
ENV HOPLITE_SERVER_CACHE=/var/lib/hoplite/cache \
    HOPLITE_HARA_STORE_PATH=/var/lib/hoplite/hara-store.sqlite \
    HOPLITE_HARA_STORE_MAX_VALUE_BYTES=8388608 \
    HOPLITE_HARA_STORE_MAX_RECEIPT_BYTES=1048576 \
    HOPLITE_WORKERS=auto
VOLUME ["/var/lib/hoplite"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 CMD curl --fail --silent http://127.0.0.1:8080/hello >/dev/null || exit 1
ENTRYPOINT ["hoplite-server"]
CMD ["/app"]
