#!/bin/sh

cargo run -p plane-cli -- --nats=nats://127.0.0.1 "$@"
