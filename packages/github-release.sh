#!/bin/bash
# Build the uinitconverter RPMs (the five core packages and the standalone tools)
# and collect them for a GitHub release. The skarnet.org packages are not
# in it: users build them with dist/rpm/build-skarnet-rpms.sh.
#
#   packages/github-release.sh [OUTDIR]      # default: ~/uinitconverter-release
#
# OUTDIR/assets gets the binary RPMs (no -devel or debuginfo packages) and
# SHA256SUMS; OUTDIR/NOTES.md is the release text with the install commands.
# Attach every file in OUTDIR/assets to the release. The RPMs are built for
# this machine's Fedora release and architecture. The tree must be committed
# (the release names the commit). Nothing is installed and no sudo is needed.
# See packages/README-us.md ("GitHub releases").
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
OUT=${1:-$HOME/uinitconverter-release}
GITHUB_REPO=shandyCho/useless-init-converter

die() { echo "error: $*" >&2; exit 1; }
command -v sha256sum >/dev/null || die "sha256sum missing: sudo dnf install coreutils"
[ -z "$(git -C "$REPO" status --porcelain)" ] || die "uncommitted changes in $REPO: commit them first"
COMMIT=$(git -C "$REPO" rev-parse --short HEAD)
VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml")
[ -n "$VERSION" ] || die "no version in $REPO/Cargo.toml [workspace.package]"
TAG=v$VERSION
DIST=$(rpm --eval '%{?fedora:fc%{fedora}}%{!?fedora:unknown}')
ARCH=$(uname -m)

# A fresh build directory, so only this run's packages are collected.
WORK=$OUT/work
rm -rf "$WORK" "$OUT/assets"
mkdir -p "$WORK" "$OUT/assets"

"$HERE/build-rpms.sh" "$WORK/core"
for b in "$REPO"/standalone/*/build-rpm.sh; do
    [ -x "$b" ] || continue
    "$b" "$WORK/standalone"
done

find "$WORK"/core/rpms "$WORK"/standalone/rpms -name '*.rpm' \
    ! -name '*.src.rpm' ! -name '*-devel-*' ! -name '*-debuginfo-*' ! -name '*-debugsource-*' \
    -exec cp {} "$OUT/assets/" \;
(cd "$OUT/assets" && sha256sum -- *.rpm >SHA256SUMS)

{
    echo "> **This software is AI slop: you install and run it at your own risk.**"
    echo ">"
    echo "> **이 소프트웨어는 AI 슬롭으로 설치와 실행에 대한 책임은 사용자에게 있습니다.**"
    echo
    echo "uinitconverter $VERSION ($COMMIT), RPMs for Fedora ${DIST#fc} $ARCH."
    echo
    echo "First build and install the skarnet.org packages (skalibs, execline, s6,"
    echo "s6-rc, s6-linux-init, s6-frontend, mdevd), which this release does not include."
    echo "Download these tarballs from their pages on https://skarnet.org/software/"
    echo "into ~/skarnet-sources (see the README's installation section):"
    echo
    while read -r name ver _; do
        case $name in ''|'#'*) continue ;; esac
        echo "- $name-$ver.tar.gz"
    done <"$REPO/dist/skarnet.versions"
    echo
    echo "then build them (the script checks the tarballs' SHA-256 sums):"
    echo
    echo '```sh'
    echo "sudo dnf install git rpm-build gcc make cpio"
    echo "git clone --branch $TAG https://github.com/$GITHUB_REPO.git"
    echo "useless-init-converter/dist/rpm/build-skarnet-rpms.sh"
    echo "sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm"
    echo '```'
    echo
    echo "Then download every .rpm file and SHA256SUMS of this release into an empty"
    echo "directory and run:"
    echo
    echo '```sh'
    echo "sha256sum -c SHA256SUMS"
    echo "sudo dnf install ./*.rpm        # an update: sudo dnf upgrade ./*.rpm"
    echo "uinitconverter init --dry-run"
    echo "sudo uinitconverter init"
    echo '```'
    echo
    echo "With the GitHub CLI: gh release download $TAG -R $GITHUB_REPO"
    echo
    echo "Packages:"
    echo
    for f in "$OUT"/assets/*.rpm; do echo "- $(basename "$f")"; done
} >"$OUT/NOTES.md"

echo
echo "==> $(find "$OUT/assets" -name '*.rpm' | wc -l) RPMs in $OUT/assets, notes in $OUT/NOTES.md"
echo "Publish (where gh is logged in; or on GitHub: Releases > Draft a new release,"
echo "tag $TAG on $COMMIT, paste NOTES.md, attach every file in $OUT/assets):"
echo "  gh release create $TAG -R $GITHUB_REPO --target $(git -C "$REPO" rev-parse HEAD) \\"
echo "      --title \"uinitconverter $VERSION\" --notes-file $OUT/NOTES.md $OUT/assets/*"
