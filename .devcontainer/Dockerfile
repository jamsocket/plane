FROM mcr.microsoft.com/devcontainers/rust:1-bullseye

# Install NATS
RUN curl -L https://github.com/nats-io/natscli/releases/download/v0.0.35/nats-0.0.35-amd64.deb -o nats.deb && dpkg -i nats.deb

USER vscode

# Install sqlite3
RUN apt-get update && apt-get install sqlite3

# Allow pushing directly to new branches.
RUN git config --global --add push.autoSetupRemote true

# Update Rust version to latest.
RUN rustup update stable

# Install useful crates
RUN cargo install cargo-workspaces
RUN cargo install cargo-outdated
RUN cargo install sqlx-cli

# Install cargo nextest
RUN curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin

