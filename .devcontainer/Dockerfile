FROM mcr.microsoft.com/devcontainers/rust:1-bullseye

RUN rustup update stable

RUN cargo install cargo-workspaces

RUN curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin

