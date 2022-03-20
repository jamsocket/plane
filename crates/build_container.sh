#!/bin/sh

set -e

if [ $# -lt 1 ]
  then
    echo "Usage: ./build.sh <command>"
    exit 1
fi

cargo build --release

TEMPDIR=$(mktemp -d)
BINNAME="${1}"
cp "target/release/${BINNAME}" "${TEMPDIR}/"

docker build \
  -f "./Dockerfile" \
  "${TEMPDIR}" \
  --build-arg \
  BINNAME="${BINNAME}" \
  --tag "${IMAGE}"
