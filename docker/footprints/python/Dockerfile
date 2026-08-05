FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --home /nonexistent pythonapp
COPY docker/footprints/python/server.py /app/server.py
USER pythonapp
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD curl --fail --silent http://127.0.0.1:8080/hello >/dev/null || exit 1
ENTRYPOINT ["python3", "/app/server.py"]
