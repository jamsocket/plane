#!/bin/bash

openssl req -x509 -nodes -days 3650 -newkey rsa:2048 -keyout localhost.key -out localhost.crt \
    -subj "/C=US/ST=YourState/L=YourCity/O=YourOrganization/OU=YourUnit/CN=localhost"

# Produce a JSON file that includes the certificate and key.
# We need to replace newlines with \n so that the JSON file is valid.
cat <<EOF > localhost.json
{
    "cert": "$(cat localhost.crt | tr '\n' '\t' | sed 's/\t/\\n/g' )",
    "key": "$(cat localhost.key | tr '\n' '\t' | sed 's/\t/\\n/g' )"
}
EOF
