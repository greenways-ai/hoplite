FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends nginx-extras curl \
    && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /tmp/nginx/client /tmp/nginx/proxy /tmp/nginx/fastcgi /tmp/nginx/uwsgi /tmp/nginx/scgi \
    && chown -R www-data:www-data /tmp/nginx
COPY docker/footprints/lua/nginx.conf /etc/nginx/nginx.conf
USER www-data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD curl --fail --silent http://127.0.0.1:8080/hello >/dev/null || exit 1
ENTRYPOINT ["nginx", "-g", "daemon off;"]
