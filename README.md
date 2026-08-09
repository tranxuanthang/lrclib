# LRCLIB

## About This Project

LRCLib is a **non-commercial, volunteer-driven** project dedicated to providing synchronized lyrics for music enthusiasts worldwide. This service is maintained by volunteers who are passionate about music and believe in making lyrics accessible to everyone.

### Non-Commercial Nature

This project is:
- **Non-profit**: We do not generate revenue or profit from this service
- **Volunteer-driven**: All contributions are made by volunteers in their free time
- **Community-focused**: Built for the benefit of the music community

### Legal Compliance

We are committed to operating within legal boundaries:
- We respect copyright holders' rights
- We have a DMCA compliance policy (see [DMCA.md](DMCA.md))
- We promptly respond to legitimate takedown requests
- We encourage only authorized submissions

### Contributing

We welcome contributions from the community! Please see our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to participate. **Important**: Only submit content you have legal authorization to share.

---

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

You have three environment variables available to tweak the database connection:

* `LRCLIB_MMAP_SIZE` (default `30000000000` - 30GB): set [SQLite's `mmap_size`](https://www.sqlite.org/mmap.html) parameter. This should be less than 75% of the available RAM. Using an excessive value might cause memory swapping, decreasing performance.
* `LRCLIB_CACHE_SIZE` (default `-1000000` - 1GB): set [SQLite's `cache_size`](https://sqlite.org/pragma.html#pragma_cache_size) parameter. Negative values are kilobytes.
* `LRCLIB_POOL_MAX_SIZE` (default `16`): set the maximum number of SQLite connections managed by the r2d2 pool. The value must be a positive integer.

Use these variables to optimize memory usage in low traffic and/or low memory situations. The default values are generously sized to be used on a production system.

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
