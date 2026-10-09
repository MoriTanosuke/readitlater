# Frontend: Caddy liefert die statischen Dateien aus und leitet /api/* ans Backend.
FROM caddy:2-alpine
COPY Caddyfile /etc/caddy/Caddyfile
COPY public/ /srv/
