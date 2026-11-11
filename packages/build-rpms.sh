#!/bin/bash
# Build the uinitconverter RPMs (uinitconverter, uinitconverter-gen, uinitconverter-runner,
# uinitconverter-timers, uinitconverter-devices, uinitconverterd) from the core workspace. They share one source
# tarball with the crates vendored, so rpmbuild needs no network.
#
#   packages/build-rpms.sh [OUTDIR]          # default: ~/rpmbuild-uinitconverter
#   packages/build-rpms.sh --srpm [OUTDIR]   # source RPMs only
#   packages/build-rpms.sh [--shared-target] [--nocheck] [--test] [OUTDIR]
#
# For RPMs only tested (in a VM, say), never published, two options make
# a rebuild much faster:
#   --shared-target  every spec builds from one copy of the source at a
#                    fixed path into one target directory, both under
#                    OUTDIR/cargo-target and kept for the next build, so
#                    cargo compiles a crate once, not once per spec and build;
#                    no debuginfo or debugsource packages
#   --nocheck        rpmbuild --nocheck: no %check (no cargo test in rpmbuild)
#   --test           both
# Published RPMs are built with neither (github-release.sh).
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
SRPM=0 SHARED=0 NOCHECK=0
while [ $# -gt 0 ]; do
    case $1 in
        --srpm) SRPM=1 ;;
        --shared-target) SHARED=1 ;;
        --nocheck) NOCHECK=1 ;;
        --test) SHARED=1 NOCHECK=1 ;;
        -*) echo "error: unknown option $1" >&2; exit 2 ;;
        *) break ;;
    esac
    shift
done
OUT=${1:-$HOME/rpmbuild-uinitconverter}
PACKAGES=(uinitconverter-runner uinitconverterd uinitconverter-timers uinitconverter-devices uinitconverter-gen uinitconverter)

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
if [ "$SRPM" = 1 ] && [ $((SHARED + NOCHECK)) -gt 0 ]; then
    die "--shared-target, --nocheck and --test build binary RPMs; not with --srpm"
fi
# The specs link target to it (%{uinitconverter_target}, unset otherwise).
# No debuginfo or debugsource packages then: the sources are outside the
# package's build directory, so debugsource would be empty (an rpmbuild error).
[ "$SHARED" = 1 ] && TOP+=(--define "uinitconverter_target $OUT/cargo-target" --define "debug_package %{nil}") \
    && mkdir -p "$OUT/cargo-target"
[ "$NOCHECK" = 1 ] && TOP+=(--nocheck)

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

[ "$SHARED" = 1 ] && echo "==> --shared-target: $OUT/cargo-target"
[ "$NOCHECK" = 1 ] && echo "==> --nocheck: no %check, no cargo test"
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
