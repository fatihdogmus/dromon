# OpenSSH sshd fixture for Dromon's integration tests (latest OpenSSH packaged by Alpine).
# sftp-server here advertises posix-rename@openssh.com, limits@openssh.com and copy-data.
#
# Users (see sshd_config for how each one logs in):
#   key  public key (.keys/id_ed25519), no password; owns the seed data
#   pw   password "dromon-test-pw" only
#   kbd  two factors: public key (.keys/id_ed25519_enc) then password "dromon-test-pw"
#   enc  public key with an encrypted private key (.keys/id_ed25519_enc, passphrase
#        "dromon-test-passphrase"), no password
# The passwords are fixed test values; the container only listens on 127.0.0.1.
FROM alpine:3.22

# A locked account ("!") is refused even for public keys; "*" only disables passwords.
RUN apk add --no-cache openssh-server openssh-sftp-server \
    && for u in key pw kbd enc; do adduser -D -s /bin/sh "$u"; done \
    && printf 'key:*\nenc:*\n' | chpasswd -e \
    && printf 'pw:dromon-test-pw\nkbd:dromon-test-pw\n' | chpasswd -c sha512

COPY sshd_config /etc/ssh/sshd_config
COPY entrypoint.sh seed.sh /usr/local/bin/
RUN chmod 755 /usr/local/bin/entrypoint.sh /usr/local/bin/seed.sh

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
