FROM lscr.io/linuxserver/firefox:latest

#RUN apk update
#RUN apk add bind-tools
#RUN apk add dnsmasq

# Set home page.
RUN echo "firefox https://plane.dev" > defaults/autostart

# Override default preferences.
COPY firefox-common-config/firstrun.js /usr/lib/firefox/browser/defaults/preferences/firstrun.js

# Copy init file, which starts the DNS server.
# COPY guac-firefox/custom-cont-init.d /custom-cont-init.d

# This adds the certificate to Alpine's certificate store, where
# curl will find it.
COPY auth/ca-cert.pem /usr/local/share/ca-certificates/plane.pem
RUN update-ca-certificates

# Install certificate for Firefox.
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/plane.crt
COPY firefox-common-config/policies.json /usr/lib/firefox/distribution/policies.json
