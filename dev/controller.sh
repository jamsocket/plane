#!/bin/sh

PLANE_LOG_JSON=true \
    cargo run -- \
    controller \
    --db postgres://postgres@localhost \
    --default-cluster localhost:9090 \
    "$@"
