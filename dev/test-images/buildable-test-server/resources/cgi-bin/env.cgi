#!/bin/sh
#take one env var from query string of forme http://host/cgi-bin/env.cgi/<<varname>> 
ENV_VAR_NAME=$(echo "${PATH_INFO}" | tail -c +2)
ENV_VAR=$(eval echo \$$(echo "${ENV_VAR_NAME}"))

cat <<EOF
Content-Type: text/plain

$ENV_VAR
EOF
