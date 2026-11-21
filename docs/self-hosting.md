# Self-hosting

How to run Jewelcase, configure it, and look after its data.

> [!WARNING]
> There is no release image yet. Today Jewelcase runs from source with the development setup,
> which only the computer running it can reach. Until the first release, an update can also
> require starting over with an empty database.

## Running it

You need [Docker](https://docs.docker.com/get-docker/) with Compose, a folder of music, and a
few minutes for the first build.

```sh
git clone https://github.com/USU-CSCapstone/MusicStreaming.git jewelcase
cd jewelcase
echo "JEWELCASE_MUSIC=/path/to/your/music" > .env
docker compose up --detach
```

Open <http://localhost:5173> once the build finishes, and **create the owner account straight
away**. Until setup is done, anyone who can reach the server can claim it, including a web page
open in your browser.

| Command | What it does |
|---|---|
| `docker compose up --detach` | Starts Jewelcase in the background |
| `docker compose logs --follow server` | Shows the server's log |
| `docker compose down` | Stops it. Your account, library, and plugins are kept. |
| `docker compose down --volumes` | Stops it and **deletes everything Jewelcase has saved**, and the build, so the next start compiles again |
| `docker compose down && git pull && docker compose up --detach --build` | Updates to the latest code |

If the server fails to start or misbehaves after an update, its database may be from an older
version it can't upgrade. Start fresh with `docker compose down --volumes`. From the first
release on, updates will upgrade the database on their own.

## Your music

Albums, artists, and tracks come from your files' tags. The formats Jewelcase reads, the tags it
uses, and where to put cover art and lyrics are in [Organizing your music](organizing-music.md).

**Jewelcase never changes your music.** It can run with the music mounted read-only. The
development setup mounts it writable so plugins you allow can save into it, such as the lyrics
plugin saving `.lrc` files. Without write access, those plugins can't save anything.

**Changes are picked up as they happen.** Jewelcase watches the folder, and also rescans it once
a day for network shares (NFS, SMB) where watching doesn't work. A very large library on Linux
may hit the system's limit on watched folders; the log says so, and raising
`fs.inotify.max_user_watches` on the host fixes it.

**On Windows, new music isn't picked up as it's added.** Docker Desktop shares a folder on a
Windows drive in a way that doesn't tell Jewelcase when it changes, so music you add appears
with the next rescan. To see it sooner, an admin can press **Scan now** on the Libraries page.
The server's log warns about this at startup, as it does for network shares. Docker's advice is
to keep files that need change notifications in the WSL filesystem instead; see its
[WSL best practices](https://docs.docker.com/desktop/features/wsl/best-practices/).

## Configuration

Jewelcase is configured with environment variables. Every one has a working default, and a
mistake, including a misspelt `JEWELCASE_` name, stops the server at startup with a message
saying what's wrong.

| Variable | Default | What it sets |
|---|---|---|
| `JEWELCASE_MUSIC` | none | The music folder. On first start it becomes the library "Music". |
| `JEWELCASE_DATA_DIR` | `/data` | Where Jewelcase keeps its own data ([below](#your-data)) |
| `JEWELCASE_PORT` | `8080` | The port the server listens on |
| `JEWELCASE_BASE_PATH` | none | A path to serve under, such as `/music`. For now only the API can be served under one, not the web app. |
| `JEWELCASE_LOG` | `info` | How much to log: `error`, `warn`, `info`, `debug`, or `trace` |
| `JEWELCASE_TRUSTED_PROXIES` | `127.0.0.0/8, ::1` | Reverse proxies to believe about the client's address, as addresses or networks separated by commas |
| `JEWELCASE_WEB_DIR` | `/usr/share/jewelcase/web` | The web app's build, served beside the API. Without one at the default, only the API is served; a folder you name must be there. |
| `JEWELCASE_OWNER_RESET` | off | Set to `1` to give the owner a new password reset link at startup, for an owner who is locked out ([below](#recovering-the-owners-password)). Any other value stops the server. |

With the development setup, `JEWELCASE_MUSIC` in `.env` picks the folder that's mounted as the
library; point it somewhere else and the library shows that folder's music instead. Of the
others, only `JEWELCASE_LOG` can be changed, since the setup depends on the rest. Set it in a
`compose.override.yaml` beside `compose.yaml`, which Compose
reads on its own:

```yaml
services:
  server:
    environment:
      JEWELCASE_LOG: debug
```

## Other people

The owner and the admins they choose invite people from **Invites** in the app and manage
their accounts from **Accounts**. An invite is a single-use link that works for 1 to 30 days. If
the owner loses their own password, the person who runs the server recovers it
([below](#recovering-the-owners-password)).

## Recovering the owner's password

The owner's password can't be reset from inside Jewelcase, so if it is lost, the person who
runs the server does it. Set `JEWELCASE_OWNER_RESET=1` and restart. The server logs a link, a
path ending in `/reset#` and a code, and the owner opens it on the server's address to choose a
new password. It works for 24 hours, and each start with the setting on makes a new link and
cancels the last. Then remove the setting and restart, so it doesn't issue a link every time.

## Your data

Everything Jewelcase saves is in its data directory, and nowhere else:

| Folder | What's in it | Back it up? |
|---|---|---|
| `state/` | The database (your library's index and audio analysis, accounts, devices, play history, plugin settings) and installed plugins | **Yes** |
| `cache/` | Resized artwork | No, it is rebuilt as needed |

Put the data directory on fast storage, such as an SSD. The music can be on slow disks or a
network share, but the data directory decides how fast Jewelcase feels. It must not be inside
your music folder, or the other way round, and only one server can use it at a time.

**To back up**, stop the server and copy `state/`. With the development setup, the data is in a
Docker volume:

```sh
docker compose stop server
docker run --rm -v jewelcase_data:/data:ro -v "$PWD":/backup debian \
  tar czf /backup/jewelcase-state.tar.gz -C /data state
docker compose start server
```

**To restore**, put `state/` back in an empty data directory and start the server. It picks up
whatever changed in your music while it was away, and only opens the files that changed.

```sh
docker compose down
docker volume rm jewelcase_data
docker run --rm -v jewelcase_data:/data -v "$PWD":/backup debian \
  tar xzf /backup/jewelcase-state.tar.gz -C /data
docker compose up --detach
```

## Not supported yet

- A release image to run on a home server
- Running behind a reverse proxy
- Upgrading the database from older development versions
- Backing up while the server runs
- More than one library
- Converting formats a browser can't play
- Serving the web app under a base path
