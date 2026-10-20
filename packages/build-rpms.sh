#!/bin/bash
# Build the sdcompat RPMs (sdcompat, sdcompat-gen, sdcompat-runner,
# sdcompat-timers, sdcompatd) from the core workspace. They share one source
# tarball with the crates vendored, so rpmbuild needs no network.
#
#   packages/build-rpms.sh [OUTDIR]        # default: ~/rpmbuild-sdcompat
#
# Nothing is installed and no sudo is needed. Afterwards:
#   sudo dnf install OUTDIR/rpms/*.rpm
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
OUT=${1:-$HOME/rpmbuild-sdcompat}
PACKAGES=(sdcompat-runner sdcompatd sdcompat-timers sdcompat-gen sdcompat)

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build"
command -v cargo >/dev/null || die "cargo missing: sudo dnf install cargo rust"

VERSION=$(rpmspec -q --srpm --qf '%{version}' "$HERE/sdcompat.spec")
for p in "${PACKAGES[@]}"; do
    v=$(rpmspec -q --srpm --qf '%{version}' "$HERE/$p.spec")
    [ "$v" = "$VERSION" ] || die "$p.spec has version $v, sdcompat.spec $VERSION: they share one tarball"
done

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms}
TOP=(--define "_topdir $OUT")

echo "==> Making sdcompat-$VERSION.tar.gz with vendored crates"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
src=$stage/sdcompat-$VERSION
mkdir -p "$src/.cargo"
cp -a "$REPO/Cargo.toml" "$REPO/Cargo.lock" "$REPO/crates" "$REPO/dist" "$src/"
(cd "$src" && cargo vendor --locked vendor >"$src/.cargo/config.toml")
tar czf "$OUT/SOURCES/sdcompat-$VERSION.tar.gz" -C "$stage" "sdcompat-$VERSION"

for p in "${PACKAGES[@]}"; do
    echo "==> Building $p"
    cp "$HERE/$p.spec" "$OUT/SPECS/"
    rpmbuild "${TOP[@]}" -bb "$OUT/SPECS/$p.spec" >"$OUT/$p.log" 2>&1 \
        || die "$p failed, see $OUT/$p.log"
done
# debuginfo and debugsource stay in RPMS/.
find "$OUT/RPMS" -name '*.rpm' ! -name '*-debuginfo-*' ! -name '*-debugsource-*' -exec cp {} "$OUT/rpms/" \;
echo "==> RPMs in $OUT/rpms:"
ls "$OUT/rpms"
