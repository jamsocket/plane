FROM debian:11-slim
RUN apt-get -y update && apt-get -y install firefox-esr ca-certificates

#RUN echo "firefox https://plane.dev" > defaults/autostart

# Override default preferences.
COPY firefox-common-config/firstrun.js /usr/lib/firefox/browser/defaults/preferences/firstrun.js

# Copy init file, which starts the DNS server.
#COPY guac-firefox/custom-cont-init.d /custom-cont-init.d

# This adds the certificate to Alpine's certificate store, where
# curl will find it.
COPY auth/ca-cert.pem /usr/local/share/ca-certificates/plane.crt
RUN update-ca-certificates

# Install certificate for Firefox.
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/plane.crt
COPY firefox-common-config/policies.json /etc/firefox/policies/policies.json

ENTRYPOINT ["firefox"]
