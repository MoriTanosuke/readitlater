# syntax=docker/dockerfile:1

# ---- Build: Alpine liefert nativ musl, das Ergebnis ist ein statisches Binary ----
FROM rust:1-alpine AS build
RUN apk add --no-cache gcc musl-dev
WORKDIR /src

# 1) Nur Abhängigkeiten bauen (eigener Layer, wird gecacht)
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

# 2) Eigentlichen Code bauen
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked \
    && mkdir /data

# ---- Laufzeit: minimal, ohne Shell, als Nicht-Root-Benutzer ----
FROM gcr.io/distroless/static:nonroot
COPY --from=build /src/target/release/readlater-backend /app/readlater-backend
# Datenverzeichnis gehört dem Laufzeitbenutzer (uid 65532); ein benanntes Volume erbt das.
COPY --from=build --chown=65532:65532 /data /data

ENV DATABASE_PATH=/data/app.db \
    BIND_ADDR=0.0.0.0:3000

VOLUME /data
EXPOSE 3000
USER 65532:65532

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/readlater-backend", "--healthcheck"]

ENTRYPOINT ["/app/readlater-backend"]
