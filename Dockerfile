# Development images now; the production image joins them later. Every stage runs on the same
# Debian release with its ffmpeg package, so scanning and playback behave in development as
# they will for users.

# ───────────────────────────── Development ─────────────────────────────

# The server, rebuilt and restarted by `docker compose up --watch` whenever the Rust changes.
# Compose mounts the checkout at /src.
FROM rust:1-slim-trixie AS server-dev
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ffmpeg \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The pinned toolchain, installed once here rather than on every start.
COPY rust-toolchain.toml .
RUN rustup toolchain install
# Outside the source, in a volume, so builds stay incremental across restarts and rebuilds.
ENV CARGO_TARGET_DIR=/target
CMD ["cargo", "run", "--locked", "--package", "jewelcase-server"]

# The web app on Vite's dev server, which reloads the page as the source changes and forwards
# the API to the server.
FROM node:26-trixie-slim AS web-dev
RUN npm install --global pnpm@11
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml web/.npmrc ./
RUN pnpm install --frozen-lockfile
COPY web .
# The mock validates plugin files with the pack tool, which it imports from beside the app.
COPY tools /src/tools
CMD ["pnpm", "dev", "--host"]
