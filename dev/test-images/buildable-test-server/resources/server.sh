#!/bin/sh
export EXIT_TIMEOUT
export EXIT_CODE

#kill the server after $EXIT_TIMEOUT if it is set
[ -n "${EXIT_TIMEOUT}" ] && { sleep "${EXIT_TIMEOUT}" && kill -USR1 "$$" ; } &
busybox httpd -f -p 8080 -c ./httpd.conf -h . &
trap "kill $!" USR1
wait
exit "${EXIT_CODE:-0}"

