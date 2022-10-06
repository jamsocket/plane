FROM lscr.io/linuxserver/firefox:latest

RUN apk add bind-tools
RUN apk add dnsmasq

# Set home page.
RUN echo "firefox https://plane.dev" > defaults/autostart

RUN echo 'pref("datareporting.policy.firstRunURL","");' > /usr/lib/firefox/browser/defaults/preferences/firstrun.js

COPY firefox/custom-cont-init.d /custom-cont-init.d

# This adds the certificate to Alpine's certificate store, where
# curl will find it.
COPY auth/ca-cert.pem /usr/local/share/ca-certificates/plane.pem
RUN update-ca-certificates

# Install certificate for Firefox.
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/plane.crt
COPY firefox/policies.json /usr/lib/firefox/distribution/policies.json
