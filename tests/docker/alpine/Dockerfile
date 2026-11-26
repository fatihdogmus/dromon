# Server matrix variant `alpine` (dromon-testkit `Variant::Alpine`, DROMON_SERVER_MATRIX=1):
# Alpine's OpenSSH on musl, a second distribution. Alpine's sshd is built without PAM, so `kbd`
# (keyboard-interactive through PAM, TOTP) can't log in here; the public-key users (`key`, `enc`)
# and the password users (`pw`, `keypw`, checked against /etc/shadow) work as in ../Dockerfile.
#
# The same entrypoint.sh and seed.sh as ../Dockerfile (the testkit adds them to this build
# context; they run under busybox), with an sshd_config of its own (alpine/sshd_config).
#
# Pinned to one Alpine release; bump the tag to follow the latest.
FROM alpine:3.24

# busybox adduser leaves the password "!" (locked), which a sshd without PAM refuses even for
# public keys; "*" only disables passwords.
RUN apk add --no-cache bash openssh-server openssh-sftp-server openssh-keygen socat \
    && rm -rf /etc/ssh/ssh_host_* /etc/ssh/sshd_config.d \
    && for u in key pw kbd keypw enc; do adduser -D -s /bin/sh "$u"; done \
    && printf 'key:*\nenc:*\n' | chpasswd -e \
    && mkdir /keys \
    && printf 'pw:dromon-test-pw\nkbd:dromon-test-pw\nkeypw:dromon-test-pw\n' | chpasswd

COPY sshd_config /etc/ssh/sshd_config
COPY --chmod=755 entrypoint.sh seed.sh /usr/local/bin/

EXPOSE 22 9999
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
