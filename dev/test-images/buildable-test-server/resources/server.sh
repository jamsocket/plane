#!/bin/sh
export EXIT_TIMEOUT
export EXIT_CODE
set -x

hello() {
	echo -e "HTTP/1.1 200 OK\r\nConnection: Close\r\nContent-Type: text/plain\r\n\r\nHello World!"
}

trap 'exit ${EXIT_CODE}' EXIT 
[ -n "${EXIT_TIMEOUT}" ] && { sleep "${EXIT_TIMEOUT}" && kill -EXIT "$$" ; } &
busybox httpd  -p 8080 -c ./httpd.conf -h . 
wait
