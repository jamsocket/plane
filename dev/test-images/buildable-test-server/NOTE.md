so I have to read EXIT_TIMEOUT, and EXIT_CODE from env.
wait for EXIT_TIMEOUT || 0, then exit with EXIT_CODE
in that window return all requests with "Hello World!"