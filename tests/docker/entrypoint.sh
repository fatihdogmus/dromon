#!/bin/bash
# Container start: host key, the test users' authorized keys (copied into /keys by the testkit
# before start), seed data, then sshd.
#
# Lifeline: the test process connects to port 9999 once sshd is up and keeps that connection
# open for as long as it runs. When the process exits, however it exits, the kernel closes the
# connection, socat returns, sshd is stopped and the container ends (the testkit creates it with
# AutoRemove, so Docker then deletes it). If nobody connects within 120 s the container ends too.
# DROMON_FIXTURE_KEEP=1 skips the lifeline: sshd runs until the container is stopped.
set -euo pipefail

[ -f /etc/ssh/ssh_host_ed25519_key ] || ssh-keygen -q -t ed25519 -N '' -f /etc/ssh/ssh_host_ed25519_key

authorize() {
    install -d -m 700 -o "$1" -g "$1" "/home/$1/.ssh"
    install -m 600 -o "$1" -g "$1" "$2" "/home/$1/.ssh/authorized_keys"
}
authorize key /keys/id_ed25519.pub
authorize keypw /keys/id_ed25519_enc.pub
authorize enc /keys/id_ed25519_enc.pub

su -s /bin/sh key -c /usr/local/bin/seed.sh
mkdir -p /run/sshd
/usr/sbin/sshd -t

if [ "${DROMON_FIXTURE_KEEP:-}" = 1 ]; then
    exec /usr/sbin/sshd -D -e
fi

# Listen before sshd announces itself, so the lifeline is there once the testkit sees sshd's
# "Server listening" line. "ready" tells the testkit the connection reached socat.
socat TCP-LISTEN:9999,reuseaddr,accept-timeout=120 SYSTEM:'echo ready; exec cat >/dev/null' &
lifeline=$!
/usr/sbin/sshd -D -e &
sshd=$!
wait -n "$lifeline" "$sshd" || true
echo "dromon-fixture: lifeline closed (test process gone), stopping sshd" >&2
kill "$sshd" 2>/dev/null || true
wait "$sshd" || true
