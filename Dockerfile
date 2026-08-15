# Debian rather than Alpine, deliberately: this image will run the archival
# logic, and BusyBox's coreutils differ from GNU's in ways that bite scripts
# written against the latter. ffmpeg joins the runtime stage when thumbnails
# and transcoding arrive.

FROM node:26-bookworm-slim AS web
WORKDIR /web
RUN npm install -g pnpm@11

# Lockfile first: dependency installs are cached until the lockfile changes.
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY web/ ./
# Types are generated from the Rust crate and committed, so the frontend build
# doesn't need a Rust toolchain — but a stale copy would fail typecheck here,
# which is the point of committing them.
RUN pnpm run build

FROM rust:1.96-bookworm AS build
WORKDIR /src

# Dependencies first, so edits to our own sources don't rebuild the world.
COPY Cargo.toml Cargo.lock ./
COPY crates/protect-api-types/Cargo.toml crates/protect-api-types/
COPY crates/protect-manager/Cargo.toml crates/protect-manager/
RUN mkdir -p crates/protect-api-types/src crates/protect-manager/src \
    && echo '' > crates/protect-api-types/src/lib.rs \
    && echo 'fn main() {}' > crates/protect-manager/src/main.rs \
    && cargo build --release --locked \
    && rm -rf crates/protect-api-types/src crates/protect-manager/src

COPY crates crates
# Cargo skips rebuilding when mtimes look unchanged, and the stubs above share
# paths with the real sources — touch them to force the real compile.
RUN touch crates/protect-api-types/src/lib.rs crates/protect-manager/src/main.rs \
    && cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Runs unprivileged. The clip directory is owned by whichever uid/gid the backup
# container writes as, so deployments add that gid via `group_add` rather than
# this image running as root.
RUN useradd --system --uid 10001 --create-home --home-dir /var/lib/protect-manager pm

COPY --from=build /src/target/release/protect-manager /usr/local/bin/protect-manager
COPY --from=web /web/dist /srv/static

ENV PM_STATIC_DIR=/srv/static \
    PM_STATE_DIR=/var/lib/protect-manager \
    PM_BIND=0.0.0.0:8642 \
    PM_BACKUP_DIR=/backup

USER pm
EXPOSE 8642
ENTRYPOINT ["/usr/local/bin/protect-manager"]
