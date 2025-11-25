FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM alpine:latest
COPY --from=builder /app/target/release/citus-membership-manager /usr/local/bin/
HEALTHCHECK --interval=1s --start-period=1s CMD test -f /healthcheck/manager-ready
CMD ["citus-membership-manager"]
