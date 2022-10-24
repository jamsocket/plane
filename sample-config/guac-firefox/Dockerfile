FROM lscr.io/linuxserver/firefox:latest

# Set home page.
RUN echo "firefox https://plane.dev" > defaults/autostart

# Override default preferences.
COPY firefox-common-config/firstrun.js /usr/lib/firefox/browser/defaults/preferences/firstrun.js
COPY auth/ca-cert.pem /usr/lib/mozilla/certificates/plane.crt
COPY firefox-common-config/policies.json /usr/lib/firefox/distribution/policies.json
