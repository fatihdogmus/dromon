#!/bin/sh
# Seed data for the integration tests, run as user `key` at container start. The 1 GiB file for
# the S4 copy-data tests is not made here: the testkit creates it on demand
# (`Fixture::big_file`, only the `--ignored` tests need it).
#
# $1: the home folder to seed, by default the image's /home/key. The testkit's native backend
# runs this script as its own key user with that user's home.
set -eu
umask 022

home=${1:-/home/key}
root=$home/fixture
mkdir -p "$root" "$home/scratch"

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
