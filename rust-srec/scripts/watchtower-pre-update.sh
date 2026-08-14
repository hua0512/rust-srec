#!/bin/sh
# Watchtower pre-update lifecycle hook
# (label: com.centurylinklabs.watchtower.lifecycle.pre-update).
#
# Gates container auto-updates on GET /api/health/idle: exit 0 lets
# Watchtower stop and recreate the container, exit 75 (EX_TEMPFAIL) skips
# this update cycle so it is retried on the next poll.
#
# Any non-200 response counts as busy — 503 (recording or pipeline job in
# progress) as well as connection failures while the app is starting up.
# Skipping an update is always safe; interrupting a recording is not.

if curl -fsS --max-time 10 "http://localhost:${API_PORT:-8080}/api/health/idle" > /dev/null; then
    exit 0
fi
exit 75
