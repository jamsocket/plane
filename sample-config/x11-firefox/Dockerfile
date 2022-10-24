FROM debian:11-slim
RUN apt-get -y update && apt-get -y install firefox-esr ca-certificates

# Override default preferences.
COPY firefox-common-config/firstrun.js /usr/lib/firefox/browser/defaults/preferences/firstrun.js


# Install certificate for Firefox.
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/plane.crt
COPY firefox-common-config/policies.json /etc/firefox/policies/policies.json

ENTRYPOINT ["firefox"]
