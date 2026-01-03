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

# Start Nginx in background
nginx

# Start Node.js server
# We use exec so that the node process receives signals
exec node .output/server/index.mjs
