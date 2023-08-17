#!/bin/sh

docker run -it --rm --name nats-main -p 4222:4222 -p 6222:6222 -p 8222:8222 nats -js
