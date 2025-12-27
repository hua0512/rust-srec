#!/bin/sh
set -e

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
