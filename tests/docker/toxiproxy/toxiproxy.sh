#!/bin/bash
# toxiproxy container start: the API on 8474, then the proxy `sshd` from 0.0.0.0:2222 to
# $UPSTREAM (the sshd container's address and port, set by the testkit), then the ready line the
# testkit waits for. The toxics themselves are added by the testkit and the tests through the API.
#
# Lifeline as in ../entrypoint.sh: the test process holds a connection to port 9999; when it
# closes (the process is gone) toxiproxy is stopped and the container ends (AutoRemove deletes
# it). Nobody connecting within 120 s ends it too. DROMON_FIXTURE_KEEP=1 skips the lifeline.
set -euo pipefail

: "${UPSTREAM:?set UPSTREAM to the sshd address, host:port}"

if [ "${DROMON_FIXTURE_KEEP:-}" != 1 ]; then
    socat TCP-LISTEN:9999,reuseaddr,accept-timeout=120 SYSTEM:'echo ready; exec cat >/dev/null' &
    lifeline=$!
fi

toxiproxy -host 0.0.0.0 -port 8474 &
proxy=$!

up=0
for _ in $(seq 100); do
    if toxiproxy-cli list >/dev/null 2>&1; then
        up=1
        break
    fi
    sleep 0.1
done
if [ "$up" != 1 ]; then
    echo "dromon-toxiproxy: the API on 8474 didn't come up" >&2
    exit 1
fi
toxiproxy-cli create --listen 0.0.0.0:2222 --upstream "$UPSTREAM" sshd >&2
echo "dromon-toxiproxy: proxy sshd 0.0.0.0:2222 -> $UPSTREAM ready" >&2

if [ "${DROMON_FIXTURE_KEEP:-}" = 1 ]; then
    wait "$proxy"
    exit
fi
wait -n "$lifeline" "$proxy" || true
echo "dromon-toxiproxy: lifeline closed (test process gone), stopping toxiproxy" >&2
kill "$proxy" 2>/dev/null || true
wait "$proxy" || true
