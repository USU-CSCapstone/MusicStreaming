# Jewelcase

A self-hosted music server that streams the collection you own like a commercial streaming
service.

Jewelcase scans your music folder and plays it in your browser, with search as you type and
control from any of your devices. It's designed to stay fast with hundreds of thousands of
tracks, even on a Raspberry Pi. Plugins add everything else, such as fetching lyrics and scrobbling.

> [!WARNING]
> Jewelcase is in early development. There is no release yet, and the API and plugin contract
> will change. See [`PLAN.md`](PLAN.md) for what's next.

## Features

- **Live library scanning:** picks up files as they're added, changed, or removed.
- **Read-only to your music:** Jewelcase keeps its own data separately and never changes your
  files. Only plugins you allow can write to them.
- **Instant search:** finds albums, artists, and songs as you type, typos included.
- **Web player:** seeking, waveforms, lyrics, and media keys and lock-screen controls.
- **Remote control:** see your signed-in devices and control what plays from any of them.
- **Plugins:** sandboxed WebAssembly plugins that an admin approves and turns on per library.
  [LRCLIB lyrics](plugins/lrclib-lyrics/) and [ListenBrainz scrobbling](plugins/listenbrainz/)
  are included as examples.

Playlists, offline listening, recommendations, multiple users, transcoding, and an Android app
are planned.

## Getting started

You need [Docker](https://docs.docker.com/get-docker/) with Compose.

```sh
git clone https://github.com/USU-CSCapstone/MusicStreaming.git jewelcase
cd jewelcase
echo "JEWELCASE_MUSIC=/path/to/your/music" > .env
docker compose up
```

The first start takes a few minutes to compile. Then open <http://localhost:5173> and **create
the owner account right away**: until you do, anyone who can reach the server can claim it.

This runs the development setup, which only your computer can reach. There's no image for a
home server yet; see [Self-hosting](docs/self-hosting.md).

## Documentation

| I want to… | Read |
|---|---|
| Run Jewelcase and configure it | [Self-hosting](docs/self-hosting.md) |
| Organize and tag my music | [Organizing your music](docs/organizing-music.md) |
| Understand how Jewelcase fits together | [Architecture](docs/architecture.md) |
| Write a plugin | [Writing a plugin](plugins/README.md) |
| Build a client or script against the API | [API reference](api/openapi.yaml) |
| Work on Jewelcase itself | [Contributing](CONTRIBUTING.md) |
| Know why it works the way it does | [Requirements](requirements/), [design notes](design/), and [decisions](adr/) |
