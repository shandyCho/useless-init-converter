#!/bin/bash
# Build RPMs for the skarnet.org packages listed in dist/skarnet.versions
# (Fedora does not package them), in that order.
#
#   dist/rpm/build-skarnet-rpms.sh [OUTDIR]          # default: ~/rpmbuild-skarnet
#   dist/rpm/build-skarnet-rpms.sh --srpm [OUTDIR]   # source RPMs only (for COPR)
#
# With --srpm the source RPMs go to OUTDIR/srpms, each with its version and
# release in the spec and the verified skarnet.org tarball inside; a
# builder such as COPR must build them in the order of skarnet.versions
# (each needs the -devel packages of the ones before it; see
# packages/copr-srpms.sh).
#
# With mock installed (and you in the "mock" group) everything is built in
# clean chroots with `mock --chain`. Otherwise rpmbuild is used, and each
# package is unpacked into OUTDIR/sysroot, which the next one builds against
# (not installed: a new skalibs soname would conflict with the installed
# execline and s6). Either way nothing is installed on the host and no sudo
# is needed.
#
# Afterwards:  sudo dnf install OUTDIR/rpms/*.rpm
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
SRPM=0
if [ "${1:-}" = --srpm ]; then
    SRPM=1
    shift
fi
OUT=${1:-$HOME/rpmbuild-skarnet}
VERSIONS=$HERE/../skarnet.versions
# Each build gets a newer Release, so dnf upgrades packages whose version did
# not change but that were rebuilt against newer libraries.
RELEASE=1.$(date +%Y%m%d%H%M)

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build gcc make"
command -v rpm2cpio >/dev/null && command -v cpio >/dev/null || die "rpm2cpio or cpio missing: sudo dnf install rpm cpio"
[ -r "$VERSIONS" ] || die "$VERSIONS missing"

ORDER=()
declare -A VER SHA256
while read -r name ver sum _; do
    case $name in ''|'#'*) continue ;; esac
    ORDER+=("$name")
    VER[$name]=$ver
    SHA256[$name]=$sum
done <"$VERSIONS"

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms,srpms}
TOP=(--define "_topdir $OUT")

echo "==> Downloading and verifying sources"
for name in "${ORDER[@]}"; do
    ver=${VER[$name]}
    tarball="$OUT/SOURCES/$name-$ver.tar.gz"
    [ -s "$tarball" ] || curl -fsSL -o "$tarball" "https://skarnet.org/software/$name/$name-$ver.tar.gz"
    echo "${SHA256[$name]}  $tarball" | sha256sum -c --quiet - || die "checksum mismatch for $tarball"
    # The spec takes its version from skarnet.versions. The sources are
    # built as released, unpatched.
    {
        printf '%%global skarnet_version %s\n%%global skarnet_release %s\n' "$ver" "$RELEASE"
        cat "$HERE/$name.spec"
    } >"$OUT/SPECS/$name.spec"
    echo "  $name-$ver ok"
done

if [ "$SRPM" = 1 ]; then
    echo "==> Source RPMs"
    rm -f "$OUT"/srpms/*.src.rpm
    for name in "${ORDER[@]}"; do
        rpmbuild "${TOP[@]}" -bs "$OUT/SPECS/$name.spec" >"$OUT/$name.srpm.log" 2>&1 \
            || die "$name failed, see $OUT/$name.srpm.log"
        find "$OUT/SRPMS" -name "$name-${VER[$name]}-$RELEASE.*src.rpm" -exec cp {} "$OUT/srpms/" \;
    done
    echo "Source RPMs in $OUT/srpms (build them in this order):"
    for name in "${ORDER[@]}"; do
        ls "$OUT/srpms/$name-${VER[$name]}-$RELEASE".*src.rpm
    done
    exit 0
fi

if command -v mock >/dev/null && id -nG | grep -qw mock; then
    echo "==> Building with mock --chain"
    srpms=()
    for name in "${ORDER[@]}"; do
        rpmbuild "${TOP[@]}" -bs "$OUT/SPECS/$name.spec" >/dev/null
        srpms+=("$(ls -t "$OUT"/SRPMS/"$name"-[0-9]*.src.rpm | head -1)")
    done
    mock --chain --localrepo="$OUT/mock-repo" "${srpms[@]}"
    find "$OUT/mock-repo" -name '*.rpm' ! -name '*.src.rpm' -exec cp {} "$OUT/rpms/" \;
else
    echo "==> Building with rpmbuild (each package is unpacked into $OUT/sysroot for the next build)"
    SYSROOT=$OUT/sysroot
    rm -rf "$SYSROOT"
    mkdir -p "$SYSROOT"
    for name in "${ORDER[@]}"; do
        echo "  building $name"
        # Drop RPMs of earlier releases so only this build is installed/copied.
        find "$OUT/RPMS" "$OUT/rpms" \( -name "$name-[0-9]*.rpm" -o -name "$name-devel-[0-9]*.rpm" \
            -o -name "$name-debuginfo-[0-9]*.rpm" -o -name "$name-debugsource-[0-9]*.rpm" \) -delete
        # --nodeps: the -devel BuildRequires are met by the sysroot, not by
        # installed packages.
        rpmbuild "${TOP[@]}" --define "skarnet_sysroot $SYSROOT" --nodeps \
            -ba "$OUT/SPECS/$name.spec" >"$OUT/$name.build.log" 2>&1 \
            || die "$name failed, see $OUT/$name.build.log"
        mapfile -t built < <(find "$OUT/RPMS" -name "$name-[0-9]*.rpm" -o -name "$name-devel-[0-9]*.rpm" | grep -v debug)
        cp "${built[@]}" "$OUT/rpms/"
        for rpm in "${built[@]}"; do
            rpm2cpio "$rpm" | (cd "$SYSROOT" && cpio -idm --quiet)
        done
    done
fi

echo
echo "RPMs in $OUT/rpms:"
ls -1 "$OUT/rpms"
echo
echo "Next: sudo dnf install $OUT/rpms/*.rpm  (then the sdcompat RPMs, then sudo sdcompat init)"
