# LRCLIB

## Introduction

LRCLIB server written in Rust with Axum and SQLite3 database.

This is the source code of [lrclib.net](https://lrclib.net) rewritten in Rust from scratch. LRCLIB is a completely free service for finding and contributing synchronized lyrics, with a easy-to-use and machine-friendly APIs.

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
