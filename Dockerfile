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

# The queue core as a WebAssembly module for the web app, which the node image cannot build.
# The CLI version must match the one in web/scripts/wasm.sh.
FROM rust:1-slim-trixie AS web-wasm
WORKDIR /src
COPY rust-toolchain.toml .
RUN rustup toolchain install
RUN cargo install wasm-bindgen-cli --version 0.2.129 --locked
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY web/scripts web/scripts
RUN web/scripts/wasm.sh

# The web app on Vite's dev server, which reloads the page as the source changes and forwards
# the API to the server.
FROM node:26-trixie-slim AS web-dev
RUN npm install --global pnpm@11
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml web/.npmrc ./
RUN pnpm install --frozen-lockfile
COPY web .
COPY --from=web-wasm /src/web/src/lib/core-wasm src/lib/core-wasm
# The mock validates plugin files with the pack tool, which it imports from beside the app.
COPY tools /src/tools
# Not `pnpm dev`, which would rebuild the module here, where there is no Rust.
CMD ["pnpm", "exec", "vite", "dev", "--host"]
