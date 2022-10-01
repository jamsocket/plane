FROM lscr.io/linuxserver/firefox:latest

RUN apk add bind-tools

COPY init-container.sh /custom-cont-init.d/init-container.sh
RUN chmod +x /custom-cont-init.d/init-container.sh
