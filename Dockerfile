# SecretAgent — distroless, non-root container (Phase 6b, ADR-20260623-phase6-milestone §6b).
# Multi-stage: build the musl-static binary in an Alpine builder, then copy it into a distroless
# static image that runs as a non-root user. The image carries NO secret — the age vault, SQLite
# store, and config live in mounted volumes (see compose.yaml).

FROM rust:1-alpine AS build
# build-base = gcc + musl-dev (rusqlite is `bundled` = compiles SQLite C; ring needs a C toolchain).
RUN apk add --no-cache build-base perl
WORKDIR /src
COPY . .
# Alpine's default target IS x86_64-unknown-linux-musl → a static binary at target/release/.
# --no-default-features drops the `voice` CLI subcommand (the container runs the gateway daemon;
# voice is attended-CLI only) and keeps the image minimal.
RUN cargo build --release --no-default-features \
    && cp target/release/secretagent /secretagent \
    && /secretagent --version

# Runtime: distroless static + nonroot. No shell, no package manager, minimal attack surface.
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /secretagent /usr/local/bin/secretagent
# The systemd-style StateDirectory: the daemon writes the vault/DB/audit here (a writable volume).
ENV SECRETAGENT_DATA_DIR=/data SECRETAGENT_CONFIG_DIR=/config
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/secretagent"]
CMD ["gateway"]
