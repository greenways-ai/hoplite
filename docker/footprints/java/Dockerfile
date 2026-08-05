FROM debian:bookworm-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends openjdk-17-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY docker/footprints/java/Main.java .
RUN javac --release 17 Main.java \
    && jar --create --file app.jar --main-class Main Main.class

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends openjdk-17-jre-headless curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --home /nonexistent javaapp
COPY --from=builder /src/app.jar /app/app.jar
USER javaapp
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD curl --fail --silent http://127.0.0.1:8080/hello >/dev/null || exit 1
ENTRYPOINT ["java", "-XX:MaxRAMPercentage=75.0", "-jar", "/app/app.jar"]
