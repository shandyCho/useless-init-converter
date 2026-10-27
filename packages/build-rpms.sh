#!/bin/bash
# Build the uinitconverter RPMs (uinitconverter, uinitconverter-gen, uinitconverter-runner,
# uinitconverter-timers, uinitconverterd) from the core workspace. They share one source
# tarball with the crates vendored, so rpmbuild needs no network.
#
#   packages/build-rpms.sh [OUTDIR]          # default: ~/rpmbuild-uinitconverter
#   packages/build-rpms.sh --srpm [OUTDIR]   # source RPMs only
#
# With --srpm the five source RPMs go to OUTDIR/srpms, each with the version
# and release in its spec and the vendored tarball inside, so they rebuild
# offline as they are (rpmbuild --rebuild, mock). packages/github-release.sh
# builds the binary RPMs with the other packages' for a GitHub release.
# Nothing is installed and no sudo is needed.
# Afterwards:
#   sudo dnf install OUTDIR/rpms/*.rpm       (first time)
#   sudo dnf upgrade OUTDIR/rpms/*.rpm       (after a rebuild)
#
# The version is the workspace's (Cargo.toml [workspace.package]). Each build
# gets a newer release, 1.YYYYMMDDHHMMSS.gCOMMIT (.dirty with uncommitted
# changes), so dnf sees a rebuild of changed code as an upgrade even when the
# version did not change. UINITCONVERTER_RELEASE=N overrides it (e.g. for a
# published release).
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
SRPM=0
if [ "${1:-}" = --srpm ]; then
    SRPM=1
    shift
fi
OUT=${1:-$HOME/rpmbuild-uinitconverter}
PACKAGES=(uinitconverter-runner uinitconverterd uinitconverter-timers uinitconverter-gen uinitconverter)

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build"
command -v cargo >/dev/null || die "cargo missing: sudo dnf install cargo rust"

VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml")
[ -n "$VERSION" ] || die "no version in $REPO/Cargo.toml [workspace.package]"
if [ -n "${UINITCONVERTER_RELEASE:-}" ]; then
    RELEASE=$UINITCONVERTER_RELEASE
else
    RELEASE=1.$(date +%Y%m%d%H%M%S)
    if commit=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null); then
        RELEASE+=.g$commit
        [ -z "$(git -C "$REPO" status --porcelain -- Cargo.toml Cargo.lock crates dist packages)" ] || RELEASE+=.dirty
    fi
fi

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms,srpms}
TOP=(--define "_topdir $OUT")

echo "==> uinitconverter $VERSION-$RELEASE: making uinitconverter-$VERSION.tar.gz with vendored crates"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
src=$stage/uinitconverter-$VERSION
mkdir -p "$src/.cargo"
cp -a "$REPO/Cargo.toml" "$REPO/Cargo.lock" "$REPO/crates" "$REPO/dist" "$src/"
(cd "$src" && cargo vendor --quiet --locked vendor)
cat >"$src/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
tar czf "$OUT/SOURCES/uinitconverter-$VERSION.tar.gz" -C "$stage" "uinitconverter-$VERSION"

for p in "${PACKAGES[@]}"; do
    {
        printf '%%global uinitconverter_version %s\n%%global uinitconverter_release %s\n' "$VERSION" "$RELEASE"
        cat "$HERE/$p.spec"
    } >"$OUT/SPECS/$p.spec"
done

if [ "$SRPM" = 1 ]; then
    # Only this build's source RPMs in srpms/: the five go to the builder
    # together (they require each other's version-release).
    rm -f "$OUT"/srpms/*.src.rpm
    for p in "${PACKAGES[@]}"; do
        echo "==> Source RPM $p"
        rpmbuild "${TOP[@]}" -bs "$OUT/SPECS/$p.spec" >"$OUT/$p.log" 2>&1 \
            || die "$p failed, see $OUT/$p.log"
        find "$OUT/SRPMS" -name "$p-$VERSION-$RELEASE.*src.rpm" -exec cp {} "$OUT/srpms/" \;
    done
    echo "==> Source RPMs in $OUT/srpms:"
    ls "$OUT/srpms"
    exit 0
fi

for p in "${PACKAGES[@]}"; do
    echo "==> Building $p"
    rpmbuild "${TOP[@]}" -bb "$OUT/SPECS/$p.spec" >"$OUT/$p.log" 2>&1 \
        || die "$p failed, see $OUT/$p.log"
done
# Only this build's RPMs in rpms/, so `dnf upgrade rpms/*.rpm` gets one set
# (the five are tied to one version-release); debuginfo and debugsource
# stay in RPMS/.
rm -f "$OUT"/rpms/*.rpm
find "$OUT/RPMS" -name "*-$VERSION-$RELEASE.*.rpm" ! -name '*-debuginfo-*' ! -name '*-debugsource-*' \
    -exec cp {} "$OUT/rpms/" \;
echo "==> RPMs in $OUT/rpms:"
ls "$OUT/rpms"
