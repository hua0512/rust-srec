#!/bin/sh
set -eu
# Keep the bundled plugin immutable, then allow explicit mounted plugin overrides.
exec /opt/streamlink/bin/streamlink \
    --plugin-dir /usr/local/share/streamlink/plugins \
    --plugin-dir "${XDG_DATA_HOME:-${HOME}/.local/share}/streamlink/plugins" "$@"
