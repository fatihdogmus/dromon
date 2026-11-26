# toxiproxy in front of the sshd fixture for the server matrix variant `toxic`
# (dromon-testkit `Variant::Toxic`, DROMON_SERVER_MATRIX=1): Shopify's toxiproxy binaries on a
# Debian base with socat for the lifeline (toxiproxy.sh). The testkit runs it next to a container
# of ../Dockerfile and points the proxy at that container's address (UPSTREAM).
#
# 2222: the proxied sshd. 8474: toxiproxy's HTTP API (the tests add toxics there).
# 9999: the lifeline, as in ../entrypoint.sh.
FROM ghcr.io/shopify/toxiproxy:2.12.0 AS toxiproxy

# Through Google's mirror of Docker Hub (no anonymous pull limit; see ../Dockerfile).
FROM mirror.gcr.io/library/debian:trixie-slim
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends socat \
    && rm -rf /var/lib/apt/lists/*
COPY --from=toxiproxy /toxiproxy /toxiproxy-cli /usr/local/bin/
COPY --chmod=755 toxiproxy.sh /usr/local/bin/

EXPOSE 2222 8474 9999
ENTRYPOINT ["/usr/local/bin/toxiproxy.sh"]
