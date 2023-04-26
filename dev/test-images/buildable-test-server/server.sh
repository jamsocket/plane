#!/bin/sh
export EXIT_TIMEOUT
export EXIT_CODE

hello() {
	echo -e "HTTP/1.1 200 OK\r\nConnection: Close\r\nContent-Type: text/plain\r\n\r\nHello World!"
}

trap 'exit ${EXIT_CODE}' USR1 
[ -n "${EXIT_TIMEOUT}" ] && { sleep "${EXIT_TIMEOUT}" && kill -USR1 "$$" ; } &

( while true; do
	  hello | busybox nc -l -p 8080
  done ) &
wait
