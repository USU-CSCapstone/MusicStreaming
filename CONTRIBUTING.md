# Contributing

How to set up Jewelcase for development, check your changes, and get them merged. Read
[Architecture](docs/architecture.md) first for how the pieces fit together.

## Development setup

You need Docker with Compose. Put the music to develop with in a `.env` file at the repository
root:

```sh
JEWELCASE_MUSIC=/path/to/music
```

Then:

```sh
docker compose up --watch   # start everything, then open http://localhost:5173
docker compose down         # stop it; add --volumes to also delete the database and build caches
```

As you save, the web app updates in the browser, and the server rebuilds and restarts. The first
start compiles the server, which takes a few minutes; later starts reuse the build. The API is
also on <http://localhost:8080>. The containers never write to your checkout.

### Without Docker

You need [rustup](https://rustup.rs), which installs the toolchain and the WebAssembly target on
first use, ffmpeg, and Node with pnpm.

```sh
# The server, with its data in ./data
JEWELCASE_MUSIC=/path/to/music JEWELCASE_DATA_DIR=./data cargo run -p jewelcase-server

# The web app, in another terminal, against that server
cd web
pnpm install
API_PROXY=http://localhost:8080 pnpm dev
```

`pnpm dev` without `API_PROXY` runs the web app against a mock of the API, with no server
needed.

### Plugins

Some Rust tests run the example plugins, so build them before `cargo test`:

```sh
plugins/lrclib-lyrics/build.sh && plugins/listenbrainz/build.sh
```

To try one, install its `plugins/<id>/target/<id>.wasm` on the **Plugins** page in the web app.
Writing your own is covered in the [plugin guide](plugins/README.md).

## Checks

Run these before opening a pull request. They run on your machine, so you need the tools
from [Without Docker](#without-docker). CI runs them again on every pull request.

| Where | Command |
|---|---|
| Repository root | `cargo fmt --check` |
| | `cargo clippy --workspace --all-targets -- -D warnings` |
| | `cargo clippy --workspace --lib --bins -- -D clippy::unwrap_used` |
| | `cargo test` |
| Each `plugins/<id>/` | `cargo fmt --check` |
| | `cargo clippy --all-targets -- -D warnings` |
| | `cargo clippy --target wasm32-wasip2 -- -D warnings` |
| | `cargo test` |
| `web/` | `pnpm lint` and `pnpm format:check` |
| | `pnpm check` |
| | `pnpm gen:api`, which must leave `src/lib/api/schema.d.ts` unchanged |
| | `pnpm test` |
| `api/` | `npx @redocly/cli@2.65.1 lint openapi.yaml` |

Code outside tests may not use `unwrap()`. Use `expect("why it can't fail")` only where failure
is impossible, and handle anything that depends on input, files, configuration, or the
environment. `unsafe` is not allowed anywhere.

## Making a change

Jewelcase is designed before it's built:

1. **Requirements** in [`requirements/`](requirements/) say what Jewelcase must do. If your
   change alters behavior, update them first. Nothing may break the
   [core principles](requirements/general.md#3-core-principles).
2. **Design notes** in [`design/`](design/) say how. Big decisions, such as a new dependency
   that shapes the stack, get an [ADR](adr/README.md).
3. **The API spec**, [`api/openapi.yaml`](api/openapi.yaml), changes before the server does.
   Then run `pnpm gen:api` in `web/` and commit the generated types.
4. **Then the code**, with tests, including a check that every new endpoint keeps libraries and
   accounts apart.

[`PLAN.md`](PLAN.md) lists what's being built next and the questions still open.

## Pull requests

- Rebase on `main` before submitting.
- One pull request per atomic change. Stack smaller pull requests for larger work.
- Keep it a draft until it's ready for review.
- In the description, say what changed and why, link the issues, and say how to test it.

## Code review

- Start with an AI review.
- Ask questions until you understand the change.
- Request changes if you want to review again.
- Approve with a comment if you *want* something changed, or with a task if you *need* it
  changed.

## Issues

- Open an issue for what you're working on, as you start.
- Track progress on the Kanban board.

## Team process

**Daily update in Slack:** what you finished or merged, what you're working on, and what you
need from others.

**Sprint planning, on Mondays:** what you *will* get done, what you *want* to get done, and what
you *didn't* get done last sprint.
