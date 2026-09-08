#!/bin/sh
set -eu
# Public liveness works with JWT authentication and follows an overridden API port.
exec curl --fail --silent --show-error --max-time 5 --noproxy '*' \
    "http://127.0.0.1:${API_PORT:-8080}/api/health/live" --output /dev/null
