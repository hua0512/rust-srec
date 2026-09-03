#!/usr/bin/env bash
# TEMPORARY diagnostic: confirm this runner's systemd-analyze honours
# --recursive-errors=no for both exit status and warning scope.
set -u
UNIT=rust-srec/rust-srec.service

echo "systemd: $(systemd-analyze --version | head -1)"
echo "distro:  $(. /etc/os-release; echo "$PRETTY_NAME")"
echo

inject() {
  python3 - "$UNIT" "$1" <<'PY'
import sys
unit, bad = sys.argv[1], sys.argv[2]
s = open(unit).read()
open('/tmp/probe.service', 'w').write(s.replace('[Install]', bad + '\n\n[Install]', 1))
PY
}

rc_of() {
  systemd-analyze verify $1 /tmp/probe.service >/dev/null 2>&1
  echo $?
}

printf '%-34s %-9s %-9s %-9s\n' "case" "default" "recur=no" "recur=yes"
for bad in "" "StartLimitIntervalSec=60" "TimeoutStopSec=notanumber" "Restart=bogusvalue" \
           "ProtectSystem=maybe" "MemoryMax=4Gigs" "SystemCallFilter=@nosuchset" \
           "RestrictAddressFamilies=AF_NOPE"; do
  if [ -z "$bad" ]; then
    cp "$UNIT" /tmp/probe.service
    label="<clean unit>"
  else
    inject "$bad"
    label="$bad"
  fi
  printf '%-34s %-9s %-9s %-9s\n' \
    "$label" "$(rc_of '')" "$(rc_of --recursive-errors=no)" "$(rc_of --recursive-errors=yes)"
done

echo
echo "--- host-unit warning leakage ---"
python3 - "$UNIT" <<'PY'
import sys
s = open(sys.argv[1]).read()
s = s.replace('Wants=network-online.target',
              'Wants=network-online.target\nWants=multi-user.target', 1)
open('/tmp/probe.service', 'w').write(s)
PY
for mode in "" "--recursive-errors=no"; do
  out=$(systemd-analyze verify $mode /tmp/probe.service 2>&1)
  echo "mode=${mode:-<default>}  lines=$(printf '%s' "$out" | grep -c .)"
  printf '%s\n' "$out" | head -5 | sed 's/^/      /'
done

echo
echo "--- missing ExecStart target (must stay visible in the log) ---"
sed 's#^ExecStart=.*#ExecStart=/nonexistent/binary#' "$UNIT" > /tmp/probe.service
systemd-analyze verify --recursive-errors=no /tmp/probe.service
echo "rc=$?"
