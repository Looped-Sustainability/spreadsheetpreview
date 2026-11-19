FROM rust:1.96.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/main.rs ./src/main.rs
RUN cargo fetch --locked
COPY src ./src
COPY assets ./assets
COPY README.md ./
ARG REVISION=development
RUN BUILD_REVISION="$REVISION" cargo build --release --locked --offline

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/spreadsheetpreview /usr/local/bin/spreadsheetpreview
COPY LICENSE assets/FONT-LICENSE.txt /usr/share/doc/spreadsheetpreview/
USER 10001:10001
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/spreadsheetpreview"]
