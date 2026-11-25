#!/usr/bin/env bash
# Builds Dromon's release binary and its Linux packages, as the release workflow does:
#
#   scripts/package-linux.sh [--arch x64|arm64] [--out DIR] [--build docker|host] [--no-arch] [--verify]
#
#   --arch    x64 (default: this machine's) or arm64
#   --out     where the packages go (default: target/packages)
#   --build   docker (default locally): build in an ubuntu:22.04 container, so the binary needs
#             only glibc 2.35 and installs on Debian 12 / Ubuntu 22.04+ whatever this machine
#             runs; host (default on CI, whose runners are ubuntu-22.04): build here
#   --no-arch skip the Arch package (CI makes it in its own job, from the .deb)
#   --verify  then install each package in clean containers and run `dromon --version`
#             (packaging/linux/verify-packages.sh)
#
# Writes Dromon-<version>-linux-<arch>.deb (packaging/linux/build-deb.sh) and
# Dromon-<version>-linux-<arch>.pkg.tar.zst (packaging/linux/build-arch.sh, Docker). The version
# is [workspace.package].version from Cargo.toml. Docker builds keep their own target folder
# (target/package-linux-<arch>) and cargo/rustup caches in Docker volumes dromon-{cargo,rustup}-<arch>.
# shellcheck source-path=SCRIPTDIR source=../packaging/linux/common.sh
. "$(dirname "$0")/../packaging/linux/common.sh"

arch=$(elf_arch /bin/sh)
out="$REPO_ROOT/target/packages"
build=docker
if [ "${CI:-}" = true ]; then build=host; fi
arch_package=1
verify=0
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) arch=${2:?--arch needs x64 or arm64}; shift 2 ;;
    --out) out=${2:?--out needs a folder}; shift 2 ;;
    --build) build=${2:?--build needs docker or host}; shift 2 ;;
    --no-arch) arch_package=0; shift ;;
    --verify) verify=1; shift ;;
    -h | --help) sed -n '2,/^\. /{/^\. /d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
done
case "$arch" in x64 | arm64) ;; *) die "--arch is x64 or arm64, not $arch" ;; esac
mkdir -p "$out"
out=$(cd "$out" && pwd)
version=$(workspace_version)
[ -n "$version" ] || die "no [workspace.package] version in Cargo.toml"
echo "Dromon $version, linux-$arch, building on: $build"

case "$build" in
  host)
    [ "$arch" = "$(elf_arch /bin/sh)" ] || die "--build host builds for this machine only; use --build docker for $arch"
    (cd "$REPO_ROOT" && cargo build --release --locked -p dromon-app)
    bash "$PACKAGING_LINUX/build-deb.sh" "$REPO_ROOT/target/release/dromon" "$version" "$out"
    ;;
  docker)
    platform=linux/$( [ "$arch" = x64 ] && echo amd64 || echo arm64)
    # The repository at /src (its own target folder), the output at /out; files come back owned by us.
    docker run --rm --platform "$platform" \
      -v "$REPO_ROOT:/src" -v "$out:/out" \
      -v "dromon-cargo-$arch:/cargo" -v "dromon-rustup-$arch:/rustup" \
      -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup -e CARGO_TARGET_DIR="/src/target/package-linux-$arch" \
      -e CARGO_TERM_COLOR=always -e CARGO_INCREMENTAL=0 \
      -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e DROMON_VERSION="$version" \
      ubuntu:22.04 bash -euo pipefail -c '
        export DEBIAN_FRONTEND=noninteractive PATH="/cargo/bin:$PATH"
        apt-get update -qq > /dev/null
        apt-get install -y -qq --no-install-recommends ca-certificates curl git build-essential dpkg-dev \
          pkg-config libxkbcommon-dev libwayland-dev > /dev/null
        if ! command -v rustup > /dev/null; then
          curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain none --profile minimal
        fi
        git config --global --add safe.directory /src
        cd /src
        rustup toolchain install
        cargo build --release --locked -p dromon-app
        status=0
        bash packaging/linux/build-deb.sh "$CARGO_TARGET_DIR/release/dromon" "$DROMON_VERSION" /out || status=$?
        chown -R "$HOST_UID:$HOST_GID" /out "$CARGO_TARGET_DIR"
        exit "$status"
      '
    ;;
  *) die "--build is docker or host, not $build" ;;
esac

deb="$out/Dromon-$version-linux-$arch.deb"
[ -f "$deb" ] || die "no $deb was made"
if [ "$arch_package" = 1 ]; then
  bash "$PACKAGING_LINUX/build-arch.sh" "$deb" "$version" "$out"
fi
if [ "$verify" = 1 ]; then
  [ "$arch_package" = 1 ] || die "--verify needs the Arch package too (drop --no-arch)"
  bash "$PACKAGING_LINUX/verify-packages.sh" "$out" "$version" "$arch"
fi
ls -l "$out"/Dromon-"$version"-linux-"$arch".*
