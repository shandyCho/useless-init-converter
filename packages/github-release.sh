#!/bin/bash
# Build every RPM a user needs (the skarnet.org packages, the five core
# packages and the standalone tools) and collect them for a GitHub release.
#
#   packages/github-release.sh [OUTDIR]      # default: ~/sdcompat-release
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
OUT=${1:-$HOME/sdcompat-release}
GITHUB_REPO=shandyCho/sdcompat

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

"$REPO/dist/rpm/build-skarnet-rpms.sh" "$WORK/skarnet"
"$HERE/build-rpms.sh" "$WORK/core"
for b in "$REPO"/standalone/*/build-rpm.sh; do
    [ -x "$b" ] || continue
    "$b" "$WORK/standalone"
done

find "$WORK"/skarnet/rpms "$WORK"/core/rpms "$WORK"/standalone/rpms -name '*.rpm' \
    ! -name '*.src.rpm' ! -name '*-devel-*' ! -name '*-debuginfo-*' ! -name '*-debugsource-*' \
    -exec cp {} "$OUT/assets/" \;
(cd "$OUT/assets" && sha256sum -- *.rpm >SHA256SUMS)

{
    echo "sdcompat $VERSION ($COMMIT), RPMs for Fedora ${DIST#fc} $ARCH."
    echo
    echo "Download every .rpm file and SHA256SUMS into an empty directory, then:"
    echo
    echo '```sh'
    echo "sha256sum -c SHA256SUMS"
    echo "sudo dnf install ./*.rpm        # an update: sudo dnf upgrade ./*.rpm"
    echo "sdcompat init --dry-run"
    echo "sudo sdcompat init"
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
echo "      --title \"sdcompat $VERSION\" --notes-file $OUT/NOTES.md $OUT/assets/*"
