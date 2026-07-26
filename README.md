# LRCLIB

## Introduction

LRCLIB server written in Rust with Axum and SQLite3 database.

This is the source code of [lrclib.net](https://lrclib.net) rewritten in Rust from scratch. LRCLIB is a completely free service for finding and contributing synchronized lyrics, with an easy-to-use and machine-friendly APIs.

## Setup

Build the project:
```
cargo build --release
```

Run the server:

```
LRCLIB_LOG=info cargo run --release -- serve --database db.sqlite3
```

Server will be available at http://0.0.0.0:3300

## Database configuration

You have two environment variables available to tweak the database connection:

* `LRCLIB_MMAP_SIZE` (default `30000000000` - 30GB): set [SQLite's `mmap_size`](https://www.sqlite.org/mmap.html) parameter. This should be less than 75% of the available RAM. Using an excessive value might cause memory swapping, decreasing performance.
* `LRCLIB_CACHE_SIZE` (default `-1000000` - 1GB): set [SQLite's `cache_size`](https://sqlite.org/pragma.html#pragma_cache_size) parameter. Negative values are kilobytes.

Use these variables to optimize memory usage in low traffic and/or low memory situations. The default values are generously sized to be used on a production system.

## MCP support

Alongside the REST API, the server exposes a read-only [Model Context Protocol](https://modelcontextprotocol.io) endpoint at `/mcp` (Streamable HTTP transport), so MCP-aware clients (e.g. Claude) can look up lyrics directly as tools instead of calling the REST API by hand. It runs on the same host/port as the rest of the API and requires no extra setup.

Tools exposed: `get_lyrics` (by track/artist/album/duration), `get_lyrics_by_id` (by numeric track id), `search_lyrics` (free-text search). Publishing/flagging lyrics is intentionally not exposed over MCP.

Example client config, pointing at a locally running server:

```json
{
  "mcpServers": {
    "lrclib": {
      "url": "http://localhost:3300/mcp"
    }
  }
}
```

## Setup with Podman/Docker

### Basic

Build the image:

```
podman build -t lrclib-rs:latest -f Dockerfile .
```

Run the container:

```
podman run --rm -it -d -v lrclib-data:/data -p 3300:3300 -e LRCLIB_LOG=info --name lrclib-rs lrclib-rs:latest
```

Server will be available at http://0.0.0.0:3300

### Access the SQLite database

Run the following command to directly interact with the database in command line:

```
podman run --rm -it -v lrclib-data:/data lrclib-rs:latest sqlite3 /data/db.sqlite3
```

### Docker Compose

A `docker-compose.yml` is included, building the image locally and bind-mounting `./lrclib-data` (containing `db.sqlite3`) to `/data`:

```
docker compose up -d --build
```

Server will be available at http://0.0.0.0:3300. Edit `docker-compose.yml` to change the port mapping or set `LRCLIB_MMAP_SIZE`/`LRCLIB_CACHE_SIZE`/`LRCLIB_MANAGE_TOKEN`/`LRCLIB_WORKERS_COUNT`.

### Quadlet

You can use Quadlet to run the Podman container in the background. It also handles auto-start the container after machine restart for you.

Create a file named `lrclib.container` at `~/.config/containers/systemd/`:

```
mkdir -p $HOME/.config/containers/systemd/
vi $HOME/.config/containers/systemd/lrclib.container
```

The example content of `lrclib.container`:

```
[Container]
Image=lrclib-rs:latest
PublishPort=3300:3300
Volume=lrclib-data:/data
ContainerName=lrclib-rs
Environment=LRCLIB_LOG=info

[Service]
Restart=always

[Install]
WantedBy=multi-user.target default.target
```

Reload the daemon:

```
systemctl --user daemon-reload
```

Start the service:

```
systemctl --user start lrclib.service
```

Check the status to see if `lrclib.service` is actually running:

```
systemctl --user status lrclib.service
```

Restart when the container image is updated:

```
systemctl --user restart lrclib.service
```

## Donation

Toss a coin to your developer?

**GitHub Sponsors (Recommended - 100% of your support goes to the developer):**

https://github.com/sponsors/tranxuanthang

**Buy Me a Coffee:**

https://www.buymeacoffee.com/thangtran

**Paypal:**

https://paypal.me/tranxuanthang98

**Monero (XMR):**

```
43ZN5qDdGQhPGthFnngD8rjCHYLsEFBcyJjDC1GPZzVxWSfT8R48QCLNGyy6Z9LvatF5j8kSgv23DgJpixJg8bnmMnKm3b7
```

**Litecoin (LTC):**

```
ltc1q7texq5qsp59gclqlwf6asrqmhm98gruvz94a48
```

## Contact

If you prefer to contact by email:

[hoangtudevops@protonmail.com](mailto:hoangtudevops@protonmail.com)
