#!/bin/sh
#take one env var from query string of forme http://host/ENV_VAR/<<varname>> 
ENV_VAR_NAME=$(awk -F'/' '$2=="ENV_VAR" { print $3 }')
ENV_VAR=$(eval echo \$$(echo "${ENV_VAR_NAME}"))

cat <<EOF
Content-Type: application/json

{"${ENV_VAR_NAME}":"${ENV_VAR}"}
EOF
