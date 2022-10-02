FROM lscr.io/linuxserver/firefox:latest

RUN apk add bind-tools
RUN apk add dnsmasq

COPY custom-cont-init.d /custom-cont-init.d

# This adds the certificate to Alpine's certificate store, where
# curl will find it.
COPY auth/ca-cert.pem /usr/local/share/ca-certificates/spawner.pem
RUN update-ca-certificates

# Install certificate for Firefox.
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/spawner.crt
COPY policies.json /usr/lib/firefox/distribution/policies.json
