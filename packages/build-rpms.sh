#!/bin/bash
# Build the sdcompat RPMs (sdcompat, sdcompat-gen, sdcompat-runner,
# sdcompat-timers, sdcompatd) from the core workspace. They share one source
# tarball with the crates vendored, so rpmbuild needs no network.
#
#   packages/build-rpms.sh [OUTDIR]        # default: ~/rpmbuild-sdcompat
#
# Nothing is installed and no sudo is needed. Afterwards:
#   sudo dnf install OUTDIR/rpms/*.rpm       (first time)
#   sudo dnf upgrade OUTDIR/rpms/*.rpm       (after a rebuild)
#
# The version is the workspace's (Cargo.toml [workspace.package]). Each build
# gets a newer release, 1.YYYYMMDDHHMMSS.gCOMMIT (.dirty with uncommitted
# changes), so dnf sees a rebuild of changed code as an upgrade even when the
# version did not change. SDCOMPAT_RELEASE=N overrides it (e.g. for a
# published release).
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
OUT=${1:-$HOME/rpmbuild-sdcompat}
PACKAGES=(sdcompat-runner sdcompatd sdcompat-timers sdcompat-gen sdcompat)

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build"
command -v cargo >/dev/null || die "cargo missing: sudo dnf install cargo rust"

VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml")
[ -n "$VERSION" ] || die "no version in $REPO/Cargo.toml [workspace.package]"
if [ -n "${SDCOMPAT_RELEASE:-}" ]; then
    RELEASE=$SDCOMPAT_RELEASE
else
    RELEASE=1.$(date +%Y%m%d%H%M%S)
    if commit=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null); then
        RELEASE+=.g$commit
        [ -z "$(git -C "$REPO" status --porcelain -- Cargo.toml Cargo.lock crates dist packages)" ] || RELEASE+=.dirty
    fi
fi

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms}
TOP=(--define "_topdir $OUT")

echo "==> sdcompat $VERSION-$RELEASE: making sdcompat-$VERSION.tar.gz with vendored crates"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
src=$stage/sdcompat-$VERSION
mkdir -p "$src/.cargo"
cp -a "$REPO/Cargo.toml" "$REPO/Cargo.lock" "$REPO/crates" "$REPO/dist" "$src/"
(cd "$src" && cargo vendor --quiet --locked vendor)
cat >"$src/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
tar czf "$OUT/SOURCES/sdcompat-$VERSION.tar.gz" -C "$stage" "sdcompat-$VERSION"

for p in "${PACKAGES[@]}"; do
    echo "==> Building $p"
    {
        printf '%%global sdcompat_version %s\n%%global sdcompat_release %s\n' "$VERSION" "$RELEASE"
        cat "$HERE/$p.spec"
    } >"$OUT/SPECS/$p.spec"
    rpmbuild "${TOP[@]}" -bb "$OUT/SPECS/$p.spec" >"$OUT/$p.log" 2>&1 \
        || die "$p failed, see $OUT/$p.log"
done
# debuginfo and debugsource stay in RPMS/.
# Only this build's RPMs in rpms/, so `dnf upgrade rpms/*.rpm` gets one set
# (the five are tied to one version-release); debuginfo stays in RPMS/.
rm -f "$OUT"/rpms/*.rpm
find "$OUT/RPMS" -name "*-$VERSION-$RELEASE.*.rpm" ! -name '*-debuginfo-*' ! -name '*-debugsource-*' \
    -exec cp {} "$OUT/rpms/" \;
echo "==> RPMs in $OUT/rpms:"
ls "$OUT/rpms"
