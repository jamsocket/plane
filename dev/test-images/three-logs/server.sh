#!/bin/sh

while true; do
	echo -e "test\ntest\ntest\n"
	cat <<-EOF  | busybox nc -l -p 8080
	HTTP/1.1 200 OK
	Content-Type: text/html
	Connection: close

	<html><body><h1>Hello, World!</h1></body></html>'
	EOF
done 
