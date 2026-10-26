#!/bin/bash
# Build the sdcompat-fdstore RPM from this project. The source tarball has
# the crates vendored, so rpmbuild needs no network.
#
#   ./build-rpm.sh [OUTDIR]        # default: ~/rpmbuild-sdcompat-standalone
#
# Nothing is installed and no sudo is needed. Afterwards:
#   sudo dnf install OUTDIR/rpms/*.rpm       (first time)
#   sudo dnf upgrade OUTDIR/rpms/*.rpm       (after a rebuild)
#
# The version is Cargo.toml's (the spec's Version must match). Each build
# gets a newer release, 1.YYYYMMDDHHMMSS.gCOMMIT (.dirty with uncommitted
# changes), so dnf sees a rebuild as an upgrade. SDCOMPAT_RELEASE=N
# overrides it (e.g. for a published release).
set -euo pipefail

NAME=sdcompat-fdstore
HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
OUT=${1:-$HOME/rpmbuild-sdcompat-standalone}

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build"
command -v cargo >/dev/null || die "cargo missing: sudo dnf install cargo rust"

VERSION=$(sed -n '/^\[package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$HERE/Cargo.toml")
[ -n "$VERSION" ] || die "no version in $HERE/Cargo.toml [package]"
SPEC_VERSION=$(sed -n 's/^Version: *//p' "$HERE/$NAME.spec")
[ "$SPEC_VERSION" = "$VERSION" ] || die "$NAME.spec has Version $SPEC_VERSION, Cargo.toml $VERSION"
if [ -n "${SDCOMPAT_RELEASE:-}" ]; then
    RELEASE=$SDCOMPAT_RELEASE
else
    RELEASE=1.$(date +%Y%m%d%H%M%S)
    if commit=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null); then
        RELEASE+=.g$commit
        [ -z "$(git -C "$HERE" status --porcelain -- Cargo.toml Cargo.lock src "$NAME.spec")" ] || RELEASE+=.dirty
    fi
fi

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms}

echo "==> $NAME $VERSION-$RELEASE: making $NAME-$VERSION.tar.gz with vendored crates"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
src=$stage/$NAME-$VERSION
mkdir -p "$src/.cargo"
cp -a "$HERE/Cargo.toml" "$HERE/Cargo.lock" "$HERE/src" "$src/"
(cd "$src" && cargo vendor --quiet --locked vendor)
cat >"$src/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
tar czf "$OUT/SOURCES/$NAME-$VERSION.tar.gz" -C "$stage" "$NAME-$VERSION"

echo "==> Building $NAME"
cp "$HERE/$NAME.spec" "$OUT/SPECS/"
rpmbuild --define "_topdir $OUT" --define "build_release $RELEASE" \
    -bb "$OUT/SPECS/$NAME.spec" >"$OUT/$NAME.log" 2>&1 \
    || die "failed, see $OUT/$NAME.log"
# rpms/ holds the last build of each standalone tool; debuginfo stays in RPMS/.
rm -f "$OUT/rpms/$NAME-"[0-9]*.rpm
find "$OUT/RPMS" -name "$NAME-$VERSION-$RELEASE.*.rpm" -exec cp {} "$OUT/rpms/" \;
echo "==> RPM in $OUT/rpms:"
ls "$OUT/rpms/$NAME-$VERSION-$RELEASE".*.rpm
