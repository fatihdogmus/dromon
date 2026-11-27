#!/usr/bin/env python3
"""Writes the update tests' fixtures (service/update_verify_tests.rs, update_tests.rs).

TEST ONLY. The keys here sign nothing but these fixtures; the release key is
crates/dromon-app/assets/release-key.pub, whose secret half lives only in the `release` environment.

  test-only-key.seed / test-only-other-key.seed   Ed25519 seeds (hex) of the two test keys
  test-only-key.pub                               key A's minisign public key
  Dromon-9.9.9-*.{dmg,msi}                        stand-in release files (a few bytes)
  SHA256SUMS                                      their checksums, sha256sum's format
  SHA256SUMS.minisig                              key A, trusted comment "dromon 9.9.9"
  SHA256SUMS.other-key.minisig                    key B (not the one the tests trust)
  SHA256SUMS.other-release.minisig                key A, but for "dromon 9.9.8"
  SHA256SUMS.legacy.minisig                       key A, minisign's legacy (not pre-hashed) kind

Minisign's formats: a public key is base64("Ed" + key id + key); a signature is
base64("ED" + key id + sign(BLAKE2b-512(file))) ("Ed" and the file itself for the legacy kind),
then the trusted comment and base64(sign(signature + trusted comment)).

Run from this folder: python3 make.py (needs the `cryptography` package). The seeds are kept so
new fixtures can be signed by the same test key. `--print` prints every file instead of writing
it ("=== name" before each).
"""

import base64
import hashlib
import os
import struct
import sys

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

PRINT = "--print" in sys.argv[1:]

HERE = os.path.dirname(os.path.abspath(__file__))
VERSION = "9.9.9"
FILES = {
    f"Dromon-{VERSION}-macos-universal.dmg": b"TEST ONLY: not a disk image, a stand-in for Dromon's update tests\n",
    f"Dromon-{VERSION}-windows-x64.msi": b"TEST ONLY: not an installer, a stand-in for Dromon's update tests\n",
}


def path(name):
    return os.path.join(HERE, name)


def key(name, key_id):
    """The test key `name` (its seed made once, then kept) and its 8-byte key id."""
    seed_file = path(f"{name}.seed")
    if not os.path.exists(seed_file):
        write(f"{name}.seed", os.urandom(32).hex() + "\n")
    if os.path.exists(seed_file):
        with open(seed_file) as f:
            seed = bytes.fromhex(f.read().strip())
    else:
        seed = bytes.fromhex(SEEDS[name])
    return Ed25519PrivateKey.from_private_bytes(seed), key_id


def public(k):
    sk, key_id = k
    pk = sk.public_key().public_bytes_raw()
    shown = "%016X" % struct.unpack("<Q", key_id)[0]
    line = base64.b64encode(b"Ed" + key_id + pk).decode()
    return f"untrusted comment: TEST ONLY minisign public key {shown}\n{line}\n"


def signature(k, data, trusted, legacy=False):
    sk, key_id = k
    if legacy:
        alg, signed = b"Ed", data
    else:
        alg, signed = b"ED", hashlib.blake2b(data, digest_size=64).digest()
    sig = sk.sign(signed)
    comment = f"trusted comment: {trusted}"
    glob = sk.sign(sig + trusted.encode())
    return "\n".join(
        [
            "untrusted comment: TEST ONLY signature from a test key",
            base64.b64encode(alg + key_id + sig).decode(),
            comment,
            base64.b64encode(glob).decode(),
            "",
        ]
    )


SEEDS = {}


def write(name, content):
    if isinstance(content, bytes):
        content = content.decode()
    if PRINT:
        if name.endswith(".seed"):
            SEEDS[name[: -len(".seed")]] = content.strip()
        sys.stdout.write(f"=== {name}\n{content}")
        return
    with open(path(name), "w") as f:
        f.write(content)


def main():
    a = key("test-only-key", bytes.fromhex("1122334455667788"))
    b = key("test-only-other-key", bytes.fromhex("8877665544332211"))
    write("test-only-key.pub", public(a))
    for name, data in FILES.items():
        write(name, data)
    sums = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in FILES.items()).encode()
    write("SHA256SUMS", sums)
    write("SHA256SUMS.minisig", signature(a, sums, f"dromon {VERSION}"))
    write("SHA256SUMS.other-key.minisig", signature(b, sums, f"dromon {VERSION}"))
    write("SHA256SUMS.other-release.minisig", signature(a, sums, "dromon 9.9.8"))
    write("SHA256SUMS.legacy.minisig", signature(a, sums, f"dromon {VERSION}", legacy=True))


if __name__ == "__main__":
    main()
