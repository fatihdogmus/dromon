#!/bin/sh
# Container start: host key, the test users' authorized keys (mounted from tests/docker/.keys),
# seed data, then sshd in the foreground.
set -eu

[ -f /etc/ssh/ssh_host_ed25519_key ] || ssh-keygen -q -t ed25519 -N '' -f /etc/ssh/ssh_host_ed25519_key

authorize() {
    install -d -m 700 -o "$1" -g "$1" "/home/$1/.ssh"
    install -m 600 -o "$1" -g "$1" "$2" "/home/$1/.ssh/authorized_keys"
}
authorize key /keys/id_ed25519.pub
authorize kbd /keys/id_ed25519_enc.pub
authorize enc /keys/id_ed25519_enc.pub

su -s /bin/sh key -c /usr/local/bin/seed.sh
touch /run/dromon-ready

exec /usr/sbin/sshd -D -e
