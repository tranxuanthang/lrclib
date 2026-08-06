FROM docker.io/rust:1.91-alpine3.22 as builder
RUN apk add --no-cache alpine-sdk
WORKDIR /usr/src/lrclib-rs
COPY . .
ENV CFLAGS="-DSQLITE_MAX_MMAP_SIZE=0x4000000000"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/lrclib-rs/target \
    cargo build --release --workspace && \
    cp target/release/lrclib /usr/local/bin/lrclib

FROM alpine:3.22
RUN apk add --no-cache sqlite
COPY --from=builder /usr/local/bin/lrclib /usr/local/bin/lrclib
RUN mkdir /data
EXPOSE 3300
CMD ["lrclib", "serve", "--port", "3300", "--database", "/data/db.sqlite3"]
