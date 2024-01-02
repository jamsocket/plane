#!/bin/sh

set -e

# Build the docker image

docker build ../ -f ./Dockerfile -t plane
