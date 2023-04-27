#!/bin/sh
export EXIT_TIMEOUT
export EXIT_CODE
set -x

[ -n "${EXIT_TIMEOUT}" ] && { sleep "${EXIT_TIMEOUT}" && kill -USR1 "$$" ; } &
busybox httpd -vf -p 8080 -c ./httpd.conf -h . &
trap "kill $!" USR1
wait
exit "${EXIT_CODE:-0}"

