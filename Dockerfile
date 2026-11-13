# --- 1. Compilation du serveur Rust -------------------------------------------------
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
# Gabarits et polices sont embarqués dans le binaire (include_str!/include_bytes!).
WORKDIR /src
COPY templates ./templates
COPY fonts ./fonts
WORKDIR /src/server
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/migrations ./migrations
COPY server/src ./src
RUN cargo build --release --locked

# --- 2. Image d'exécution -----------------------------------------------------------
# Le serveur rend lui-même les pages (gabarits embarqués) ; seul `content/articles` sert au premier import dans git.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /data --shell /usr/sbin/nologin app \
    && mkdir /data && chown app /data
COPY --from=build /src/server/target/release/justprivacy-server /usr/local/bin/justprivacy-server
WORKDIR /app
COPY content ./content
ENV SITE_SRC=/app DATA_DIR=/data BIND=0.0.0.0:8080 RUST_LOG=info
VOLUME /data
USER app
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
CMD ["justprivacy-server"]
