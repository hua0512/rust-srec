#!/bin/sh
set -e

# Configure timezone from TZ env (defaults to UTC).
# Alpine/Node/Nginx will then use /etc/localtime for local time conversions.
: "${TZ:=UTC}"
if [ -f "/usr/share/zoneinfo/$TZ" ]; then
  if ln -snf "/usr/share/zoneinfo/$TZ" /etc/localtime 2>/dev/null; then
    echo "$TZ" > /etc/timezone 2>/dev/null || true
  else
    echo "warning: unable to set /etc/localtime (insufficient permissions?)" >&2
  fi
else
  echo "warning: TZ '$TZ' not found under /usr/share/zoneinfo; leaving timezone unchanged" >&2
fi
export TZ

# Ensure Nginx run directory exists
mkdir -p /run/nginx

# Perform envsubst on the template and output to Alpine's default config location
# We only substitute BACKEND_URL to avoid breaking other nginx variables like $host
envsubst '${BACKEND_URL}' < /etc/nginx/templates/default.conf.template > /etc/nginx/http.d/default.conf

# Start Nginx in background. It keeps root because it binds port 80 and writes
# /run/nginx; its master process drops the workers that handle request data to
# the unprivileged `nginx` account by itself.
nginx

# Start the Node.js SSR server as the unprivileged `node` account. Nothing it
# does needs root, and running it that way leaves it unable to modify its own
# bundle in /app/.output or the rendered nginx configuration. Everything above
# needs root and has already run by this point.
#
# su-exec switches to the account, points HOME at its home directory, and then
# replaces itself with node, so node still ends up as PID 1 and the container's
# stop signal reaches it directly.
exec su-exec node node .output/server/index.mjs
