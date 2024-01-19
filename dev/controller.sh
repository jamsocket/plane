#!/bin/sh

cargo run -- controller --db postgres://postgres@localhost "$@" --default-cluster localhost:9090
