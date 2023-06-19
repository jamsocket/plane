#!/bin/sh

# nats-server -js

docker run -d --name nats-server -p 4222:4222 nats:latest -js

