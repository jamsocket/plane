#!/bin/sh

set -e

# Build the docker image

docker build ../ -f ./quickstart/Dockerfile -t plane-quickstart
