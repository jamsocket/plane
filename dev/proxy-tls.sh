#!/bin/sh

# get the directory of this script
DIR=$(dirname "$0")

cargo run -- \
    proxy \
    --controller-url ws://localhost:8080/ \
    --cluster "localhost:9090" \
    --cert-path "${DIR}/cert/localhost.json" \
    --https-port 9433 \
    "$@"
