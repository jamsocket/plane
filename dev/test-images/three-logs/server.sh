#!/bin/sh

echo -e "test\ntest\ntest"
export EXIT_TIMEOUT
export EXIT_CODE

#kill the server after $EXIT_TIMEOUT if it is set
[ -n "${EXIT_TIMEOUT}" ] && { sleep "${EXIT_TIMEOUT}" && kill "$$" ; } &
while true; do
	cat <<-EOF  | busybox nc -l -p 8080 > /dev/null
	HTTP/1.1 200 OK
	Content-Type: text/html
	Connection: close

	<html><body><h1>Hello, World!</h1></body></html>'
	EOF
done 
