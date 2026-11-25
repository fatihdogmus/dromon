#!/usr/bin/env bash
# Generates the fixture's test keypairs (once, into tests/docker/.keys/, which is git-ignored),
# builds and starts the sshd fixture on 127.0.0.1:${DROMON_SSH_FIXTURE_PORT:-2222} and waits
# until an SFTP login works.
#
#   bash tests/docker/setup.sh
#   DROMON_SSH_FIXTURE=1 cargo test -p dromon-ssh -p dromon-sftp -- --nocapture
#   docker compose -f tests/docker/compose.yml down
#
# Keys: id_ed25519 (no passphrase, users key) and id_ed25519_enc (passphrase
# "dromon-test-passphrase", users enc and kbd). Users and passwords: see Dockerfile.
set -euo pipefail

cd "$(dirname "$0")"
port="${DROMON_SSH_FIXTURE_PORT:-2222}"

mkdir -p .keys
chmod 700 .keys
if [ ! -f .keys/id_ed25519 ]; then
    ssh-keygen -q -t ed25519 -N '' -C dromon-fixture -f .keys/id_ed25519
fi
if [ ! -f .keys/id_ed25519_enc ]; then
    ssh-keygen -q -t ed25519 -N 'dromon-test-passphrase' -C dromon-fixture-enc -f .keys/id_ed25519_enc
fi

docker compose up -d --build --wait

for _ in $(seq 1 30); do
    if ssh -T -x -e none -o BatchMode=yes -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -i .keys/id_ed25519 -p "$port" \
        key@127.0.0.1 true </dev/null; then
        echo "sshd fixture ready on 127.0.0.1:$port (user key)"
        exit 0
    fi
    sleep 1
done
echo "sshd fixture did not accept a login on 127.0.0.1:$port" >&2
exit 1
