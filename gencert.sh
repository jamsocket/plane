#!/bin/bash

# Generate a self-signed certificate for testing. Do not use the generated cert in production!

TEMP_DIR=$(mktemp -d -t certs-XXXXX)
OUT_FILE="test-cert.json"

# Create CA key + cert
openssl req \
    -nodes \
    -x509 \
    -newkey rsa:4096 \
    -keyout "${TEMP_DIR}/ca-key.pem" \
    -out "${TEMP_DIR}/ca-cert.pem" \
    -sha256 \
    -days 3650 \
    -subj "/C=US/ST=NY/L=New York/O=Plane/OU=Org/CN=plane.test"

# Create a certificate signing request.
openssl req \
    -nodes \
    -new \
    -newkey rsa:4096 \
    -keyout "${TEMP_DIR}/site-key.pem" \
    -out "${TEMP_DIR}/site-cert.csr" \
    -subj "/C=US/ST=NY/L=New York/O=Plane/OU=Org/CN=plane.test"

echo "subjectAltName = DNS:*.plane.test" > "${TEMP_DIR}/extfile"

# Sign the certificate.
openssl x509 \
    -req \
    -in "${TEMP_DIR}/site-cert.csr" \
    -days 3650 \
    -CA "${TEMP_DIR}/ca-cert.pem" \
    -CAkey "${TEMP_DIR}/ca-key.pem" \
    -CAcreateserial \
    -out "${TEMP_DIR}/site-cert.pem" \
    -extfile "${TEMP_DIR}/extfile"

# Clean up

key=$(cat "${TEMP_DIR}/site-key.pem")
cert=$(cat "${TEMP_DIR}/site-cert.pem")

echo "{" > "${OUT_FILE}"
echo "  \"key\": \"${key//$'\n'/\\n}\"," >> "${OUT_FILE}"
echo "  \"cert\": \"${cert//$'\n'/\\n}\"" >> "${OUT_FILE}"
echo "}" >> "${OUT_FILE}"


rm -rf $TEMP_DIR
