ARG NGINX_VERSION=1.30.4

FROM rust:1.85-bookworm AS builder
ARG NGINX_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl libpcre2-dev libssl-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY hoplite hoplite
COPY hara.lang hara.lang
RUN cargo build --manifest-path hoplite/runtime/Cargo.toml --release
RUN curl -fsSLO "https://nginx.org/download/nginx-${NGINX_VERSION}.tar.gz" \
    && tar -xzf "nginx-${NGINX_VERSION}.tar.gz" \
    && cd "nginx-${NGINX_VERSION}" \
    && HOPLITE_RUNTIME_DIR=/src/hoplite/target/release \
       ./configure --prefix=/src/hoplite/target/nginx \
         --with-http_ssl_module --with-threads \
         --add-module=/src/hoplite/nginx \
    && make -j"$(nproc)" \
    && make install
RUN cargo build --manifest-path hoplite/Cargo.toml --release --features embedded-nginx --bin hoplite

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl libpcre2-8-0 libssl3 zlib1g \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --system hoplite && useradd --system --gid hoplite --home /var/lib/hoplite hoplite \
    && mkdir -p /app /var/lib/hoplite \
    && chown -R hoplite:hoplite /app /var/lib/hoplite
COPY --from=builder /src/hoplite/target/release/hoplite /usr/local/bin/hoplite
COPY --chown=hoplite:hoplite hoplite/examples /app
USER hoplite
ENV HOPLITE_STATE_DIR=/var/lib/hoplite
VOLUME ["/var/lib/hoplite"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 CMD curl --fail --silent http://127.0.0.1:8080/hello >/dev/null || exit 1
ENTRYPOINT ["hoplite"]
CMD ["serve", "foreground", "--mode", "prod", "/app"]
