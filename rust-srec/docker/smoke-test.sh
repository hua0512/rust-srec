#!/usr/bin/env bash
set -euo pipefail

image=${1:?Pass the locally built image tag}
directory=$(mktemp -d)
containers=()
cleanup() {
    for container in "${containers[@]}"; do
        docker rm -f "$container" >/dev/null 2>&1 || true
    done
    sudo rm -rf -- "$directory"
}
trap cleanup EXIT

wait_healthy() {
    local container=$1 status
    for _ in $(seq 1 36); do
        status=$(docker inspect --format '{{.State.Health.Status}}' "$container")
        if [[ "$status" == healthy ]]; then return; fi
        if [[ "$(docker inspect --format '{{.State.Running}}' "$container")" != true ]]; then break; fi
        sleep 5
    done
    docker logs "$container"
    docker inspect --format '{{json .State}}' "$container"
    return 1
}

default_container=$(docker run -d -e JWT_SECRET=runtime-image-smoke-secret-at-least-32-characters "$image")
containers+=("$default_container")
wait_healthy "$default_container"
test "$(docker exec "$default_container" id -u)" = 1000
test "$(docker exec "$default_container" id -g)" = 1000
docker exec "$default_container" sh -ec '
    test -w /app/data && test -w /app/config && test -w /app/output && test -w /app/logs
    test -w "$XDG_DATA_HOME/streamlink/plugins"
    test "$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8080/api/health)" = 401
    ffmpeg -version
    ffprobe -version
    ffplay -version
    rclone version
    BaiduPCS-Go --version
    streamlink --version
    /opt/streamlink/bin/pip check
    streamlink --loglevel debug --can-handle-url-no-redirect https://www.twitch.tv/rust_srec_smoke > /tmp/plugin-load.log 2>&1
    grep -F /usr/local/share/streamlink/plugins/twitch.py /tmp/plugin-load.log
    printf "%s\n" "<i><d p=\"1,1,25,16777215,0,0,user,0\">smoke</d></i>" > /tmp/danmaku.xml
    DanmakuFactory -i /tmp/danmaku.xml -o /tmp/danmaku.ass
    test -s /tmp/danmaku.ass
'

for name in data config output logs; do
    sudo install -d -o 12345 -g 23456 -m 0750 "$directory/$name"
done
custom_container=$(docker run -d --user 12345:23456 \
    -e JWT_SECRET=runtime-image-smoke-secret-at-least-32-characters -e API_PORT=18081 \
    --mount "type=bind,source=$directory/data,target=/app/data" \
    --mount "type=bind,source=$directory/config,target=/app/config" \
    --mount "type=bind,source=$directory/output,target=/app/output" \
    --mount "type=bind,source=$directory/logs,target=/app/logs" "$image")
containers+=("$custom_container")
wait_healthy "$custom_container"
test "$(docker exec "$custom_container" id -u)" = 12345
test "$(docker exec "$custom_container" id -g)" = 23456
docker exec "$custom_container" sh -ec '
    touch /app/data/smoke /app/config/smoke /app/output/smoke /app/logs/smoke
    touch "$XDG_DATA_HOME/streamlink/plugins/smoke"
    mkdir -p "$BAIDUPCS_GO_CONFIG_DIR"
    rclone config paths
    streamlink --can-handle-url-no-redirect https://www.twitch.tv/rust_srec_smoke
    rust-srec-healthcheck
'
test "$(sudo stat -c '%u:%g' "$directory/data/smoke")" = 12345:23456
for container in "${containers[@]}"; do
    docker stop --time 35 "$container" >/dev/null
    test "$(docker inspect --format '{{.State.ExitCode}}' "$container")" = 0
done
