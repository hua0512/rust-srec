#!/bin/sh
set -eu

# Numeric --user overrides need no passwd entry: all tool state has explicit paths.
umask 077
for directory in /app/data /app/config /app/output /app/logs "$HOME" "$XDG_DATA_HOME/streamlink/plugins" "$XDG_CACHE_HOME"; do
    if ! mkdir -p "$directory" || ! test -w "$directory"; then
        echo "Cannot write $directory as $(id -u):$(id -g); prepare the mounted directory for this UID/GID." >&2
        exit 1
    fi
done
exec "$@"
