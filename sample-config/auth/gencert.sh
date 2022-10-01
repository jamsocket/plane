#!/bin/sh

openssl req \
    -nodes \
    -x509 \
    -newkey \
    rsa:4096 \
    -keyout key.pem \
    -out cert.pem \
    -sha256 \
    -days 3650 \
    -subj "/C=US/ST=NY/L=New York/O=Spawner/OU=Org/CN=spawner.test" \
    -addext "subjectAltName = DNS:*.spawner.test"
