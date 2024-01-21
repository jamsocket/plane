#!/bin/sh

# Create CA key + cert
openssl req \
    -nodes \
    -x509 \
    -newkey rsa:4096 \
    -keyout ca-key.pem \
    -out ca-cert.pem \
    -sha256 \
    -days 3650 \
    -subj "/C=US/ST=NY/L=New York/O=Plane/OU=Org/CN=plane.test"

# Create a certificate signing request.
openssl req \
    -nodes \
    -new \
    -newkey rsa:4096 \
    -keyout site-key.pem \
    -out site-cert.csr \
    -subj "/C=US/ST=NY/L=New York/O=Plane/OU=Org/CN=plane.test"

echo "subjectAltName = DNS:*.plane.test" > extfile

# Sign the certificate.
openssl x509 \
    -req \
    -in site-cert.csr \
    -days 365 \
    -CA ca-cert.pem \
    -CAkey ca-key.pem \
    -CAcreateserial \
    -out site-cert.pem \
    -extfile extfile

# Clean up
rm \
    extfile \
    ca-cert.srl \
    ca-key.pem \
    site-cert.csr
