#!/bin/sh
# Seed data for the integration tests, run as user `key` at every container start.
# Idempotent: the mixed and many folders are rebuilt, the big file is kept once generated.
set -eu

root=/home/key/fixture
mkdir -p "$root" /home/key/scratch

# ~100 mixed entries: files, folders, symlinks (to a folder, to a file, dangling), dotfiles,
# a FIFO and an unreadable folder.
mixed="$root/mixed"
[ -d "$mixed/locked" ] && chmod 700 "$mixed/locked"
rm -rf "$mixed"
mkdir -p "$mixed"
cd "$mixed"
i=1
while [ "$i" -le 80 ]; do
    n=$(printf '%03d' "$i")
    head -c "$((i * 100))" /dev/zero >"file-$n.txt"
    i=$((i + 1))
done
i=1
while [ "$i" -le 10 ]; do
    mkdir "dir-$(printf '%02d' "$i")"
    i=$((i + 1))
done
echo inside >dir-01/inside.txt
: >empty.txt
echo spaces >"space name.txt"
echo unicode >"unicode-ğüş.txt"
echo secret >.hidden
mkdir .config
ln -s dir-01 link-dir
ln -s file-001.txt link-file
ln -s does-not-exist dangling
mkfifo pipe
mkdir locked
chmod 000 locked
# A known mtime: 2020-01-02 03:04:05 UTC = 1577934245.
TZ=UTC touch -t 202001020304.05 file-001.txt

# 10,000 empty files for paging and large-listing tests.
many="$root/many"
rm -rf "$many"
mkdir -p "$many"
cd "$many"
seq -w 1 10000 | sed 's/^/f/' | xargs touch

# 1 GiB of random data for the copy-data proof of concept (S4).
big="$root/big"
mkdir -p "$big"
if [ "$(stat -c %s "$big/big.bin" 2>/dev/null || echo 0)" != 1073741824 ]; then
    head -c 1073741824 /dev/urandom >"$big/big.bin.tmp"
    mv "$big/big.bin.tmp" "$big/big.bin"
fi
