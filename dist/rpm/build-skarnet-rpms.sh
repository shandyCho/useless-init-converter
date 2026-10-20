#!/bin/bash
# Build RPMs for skalibs, execline, s6, s6-rc, s6-linux-init and s6-frontend
# (Fedora does not package them), in dependency order.
#
#   dist/rpm/build-skarnet-rpms.sh [OUTDIR]        # default: ~/rpmbuild-skarnet
#
# With mock installed (and you in the "mock" group) everything is built in
# clean chroots with `mock --chain`; nothing is installed on the host.
# Otherwise rpmbuild is used, and each -devel package is installed with
# `sudo dnf` because the next package builds against it.
#
# Afterwards:  sudo dist/setup-fedora.sh --rpms OUTDIR/rpms
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
OUT=${1:-$HOME/rpmbuild-skarnet}
ORDER=(skalibs execline s6 s6-rc s6-linux-init s6-frontend)
declare -A SHA256=(
    [skalibs-2.15.1.0]=f9c905e74935c6fe911c7e344e3e89d5fbd2014c1a04650b524b15ce9b5635d1
    [execline-2.9.9.2]=908ed4db3a6b3a23a205d8fd4cf2a71089156f2aeae0f54656045aafad2dee32
    [s6-2.15.1.0]=eab9c46e22b66b16135f9a05ec68a0ea287d9060b84d10defaaa2caad158ab52
    [s6-rc-0.7.0.0]=bf5b8ce0da5a4ee70d642b818b61d9916a7a9b64a457595f388113e54a188688
    [s6-linux-init-1.2.0.2]=6fad014da162c0c81924197c57d16e1a75c133b34a20e423431a1b741e907b1d
    [s6-frontend-0.1.0.0]=ed7bba6f2c13dd16ebf17fda0d482f4ba4848d80f5b4d7edd22d8b60a869f994
)

die() { echo "error: $*" >&2; exit 1; }
command -v rpmbuild >/dev/null || die "rpmbuild missing: sudo dnf install rpm-build gcc make"

mkdir -p "$OUT"/{SOURCES,SPECS,SRPMS,RPMS,BUILD,rpms}
TOP=(--define "_topdir $OUT")

echo "==> Downloading and verifying sources"
for name in "${ORDER[@]}"; do
    ver=$(rpmspec -q --qf '%{version}\n' "$HERE/$name.spec" 2>/dev/null | head -1)
    [ -n "$ver" ] || ver=$(sed -n 's/^Version: *//p' "$HERE/$name.spec")
    tarball="$OUT/SOURCES/$name-$ver.tar.gz"
    want=${SHA256[$name-$ver]:-}
    [ -n "$want" ] || die "no pinned checksum for $name-$ver (update SHA256 in $0)"
    [ -s "$tarball" ] || curl -fsSL -o "$tarball" "https://skarnet.org/software/$name/$name-$ver.tar.gz"
    echo "$want  $tarball" | sha256sum -c --quiet - || die "checksum mismatch for $tarball"
    cp "$HERE/$name.spec" "$OUT/SPECS/"
    for p in "$HERE/$name"-*.patch; do
        if [ -e "$p" ]; then cp "$p" "$OUT/SOURCES/"; fi
    done
    echo "  $name-$ver ok"
done

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
    echo "==> Building with rpmbuild (each -devel package is installed for the next build)"
    for name in "${ORDER[@]}"; do
        echo "  building $name"
        # Drop RPMs of earlier releases so only this build is installed/copied.
        find "$OUT/RPMS" "$OUT/rpms" \( -name "$name-[0-9]*.rpm" -o -name "$name-devel-[0-9]*.rpm" \
            -o -name "$name-debuginfo-[0-9]*.rpm" -o -name "$name-debugsource-[0-9]*.rpm" \) -delete
        rpmbuild "${TOP[@]}" -ba "$OUT/SPECS/$name.spec" >"$OUT/$name.build.log" 2>&1 \
            || die "$name failed, see $OUT/$name.build.log"
        mapfile -t built < <(find "$OUT/RPMS" -name "$name-[0-9]*.rpm" -o -name "$name-devel-[0-9]*.rpm" | grep -v debug)
        cp "${built[@]}" "$OUT/rpms/"
        [ "$name" = s6-frontend ] || sudo dnf install -y "${built[@]}"
    done
fi

echo
echo "RPMs in $OUT/rpms:"
ls -1 "$OUT/rpms"
echo
echo "Next: sudo $HERE/../setup-fedora.sh --rpms $OUT/rpms"
