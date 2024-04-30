FROM rust:1.77.2-alpine3.19 as builder

WORKDIR /usr/src/lrclib

COPY . .

RUN apk add --no-cache alpine-sdk

RUN cargo build --release --workspace

FROM alpine:3.19

RUN apk add --no-cache sqlite

COPY --from=builder /usr/src/lrclib/target/release/lrclib /usr/local/bin/lrclib

RUN mkdir /data

EXPOSE 3300

CMD ["lrclib", "serve", "--port", "3300", "--database", "/data/db.sqlite3"]
