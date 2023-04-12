#!/bin/sh

cargo run -q -p plane-cli -- --nats=nats://127.0.0.1 "$@"
