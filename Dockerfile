FROM docker.io/rust:1.78-alpine3.19 as chef
RUN apk add --no-cache alpine-sdk
RUN cargo install cargo-chef
WORKDIR /usr/src/lrclib

FROM chef AS planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /usr/src/lrclib/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --workspace

FROM alpine:3.19
RUN apk add --no-cache sqlite
COPY --from=builder /usr/src/lrclib/target/release/lrclib /usr/local/bin/lrclib
RUN mkdir /data
EXPOSE 3300
CMD ["lrclib", "serve", "--port", "3300", "--database", "/data/db.sqlite3", "--workers-count", "3"]
