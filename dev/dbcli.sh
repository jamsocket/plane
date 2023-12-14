#!/bin/sh

cargo -q run --bin db-cli -- --db postgres://postgres@localhost "$@"
