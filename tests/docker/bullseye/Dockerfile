# Server matrix variant `bullseye` (dromon-testkit `Variant::Bullseye`, DROMON_SERVER_MATRIX=1):
# Debian bullseye's OpenSSH 8.4p1. Its sftp-server advertises posix-rename@openssh.com but
# neither limits@openssh.com (reads and writes use the 32 KiB default) nor copy-data (same-host
# copies are streamed through the app).
#
# The same users, sshd_config, pam-sshd, google_authenticator, entrypoint.sh and seed.sh as
# ../Dockerfile: the testkit adds those files of tests/docker/ to this build context.
#
# Bullseye has left the Debian mirrors (its security pool already 404s), so the packages come from
# a fixed snapshot.debian.org date: the image is reproducible. Check-Valid-Until is off because a
# snapshot's Release files expire.
FROM debian:bullseye-slim

RUN printf '%s\n' \
        'deb http://snapshot.debian.org/archive/debian/20260824T000000Z bullseye main' \
        'deb http://snapshot.debian.org/archive/debian-security/20260824T000000Z bullseye-security main' \
        >/etc/apt/sources.list \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        openssh-server openssh-sftp-server libpam-google-authenticator socat \
    && rm -rf /var/lib/apt/lists/* /etc/ssh/ssh_host_* /etc/ssh/sshd_config.d \
    && for u in key pw kbd keypw enc; do useradd -m -s /bin/sh "$u"; done \
    && usermod -p '*' key && usermod -p '*' enc \
    && mkdir /keys \
    && printf 'pw:dromon-test-pw\nkbd:dromon-test-pw\nkeypw:dromon-test-pw\n' | chpasswd

COPY sshd_config /etc/ssh/sshd_config
COPY pam-sshd /etc/pam.d/sshd
COPY --chown=kbd:kbd --chmod=400 google_authenticator /home/kbd/.google_authenticator
COPY --chmod=755 entrypoint.sh seed.sh /usr/local/bin/

EXPOSE 22 9999
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
