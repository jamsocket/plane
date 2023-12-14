#!/bin/sh

cargo run -- controller --db postgres://postgres@localhost "$@"
