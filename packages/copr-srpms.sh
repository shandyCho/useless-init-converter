#!/bin/bash
# Make the source RPMs to build on Fedora COPR (or any builder that takes
# source RPMs), and the order to build them in.
#
#   packages/copr-srpms.sh [OUTDIR] [GROUP...]   # default: ~/sdcompat-srpms, all groups
#
# Groups: core (the five sdcompat packages, packages/build-rpms.sh),
# standalone (standalone/*/build-rpm.sh), skarnet (dist/rpm, the
# skarnet.org packages; needed once per skarnet.versions change).
# OUTDIR/srpms gets this run's source RPMs and OUTDIR/BUILD-ORDER.txt the
# stages: a stage needs the packages of the earlier ones in the COPR
# repository, so start it once they have built. Nothing is installed and no
# sudo is needed. See packages/README-us.md ("COPR").
set -euo pipefail

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
OUT=$HOME/sdcompat-srpms
case ${1:-} in
    ''|core|standalone|skarnet) ;;
    *) OUT=$1; shift ;;
esac
GROUPS_WANTED=("$@")
[ ${#GROUPS_WANTED[@]} -gt 0 ] || GROUPS_WANTED=(skarnet core standalone)

die() { echo "error: $*" >&2; exit 1; }
for g in "${GROUPS_WANTED[@]}"; do
    case $g in core|standalone|skarnet) ;; *) die "unknown group $g (core, standalone, skarnet)" ;; esac
done
wanted() { [[ " ${GROUPS_WANTED[*]} " == *" $1 "* ]]; }

mkdir -p "$OUT/srpms"
rm -f "$OUT"/srpms/*.src.rpm
WORK=$OUT/work
mkdir -p "$WORK"
order=$OUT/BUILD-ORDER.txt
: >"$order.tmp"

# stage N FILE...: one line of the build order.
stage_line() {
    local n=$1
    shift
    local f names=()
    for f in "$@"; do names+=("$(basename "$f")"); done
    printf 'stage %s: %s\n' "$n" "${names[*]}" >>"$order.tmp"
}

last_stage=0
if wanted skarnet; then
    "$REPO/dist/rpm/build-skarnet-rpms.sh" --srpm "$WORK/skarnet"
    # A package's stage: one after the latest stage of the -devel packages
    # it builds against (skarnet.versions lists them in build order).
    declare -A STAGE
    by_stage=()
    while read -r name _; do
        case $name in ''|'#'*) continue ;; esac
        s=1
        for dep in $(sed -n 's/^BuildRequires: *//p' "$REPO/dist/rpm/$name.spec" | tr ' ' '\n' | sed -n 's/-devel$//p'); do
            [ -n "${STAGE[$dep]:-}" ] || die "$name builds against $dep, which comes later in skarnet.versions"
            [ $((STAGE[$dep] + 1)) -le $s ] || s=$((STAGE[$dep] + 1))
        done
        STAGE[$name]=$s
        f=$(ls "$WORK/skarnet/srpms/$name"-[0-9]*.src.rpm)
        cp "$f" "$OUT/srpms/"
        by_stage[$s]+="$OUT/srpms/$(basename "$f") "
        [ "$s" -le "$last_stage" ] || last_stage=$s
    done <"$REPO/dist/skarnet.versions"
    for s in $(seq 1 "$last_stage"); do
        # shellcheck disable=SC2086 # the list is split on purpose
        stage_line "$s" ${by_stage[$s]}
    done
fi

# The Rust packages need only cargo and rust to build, so they can go into
# one stage after the skarnet ones (or alone, when those are in COPR already).
rust=()
if wanted core; then
    "$HERE/build-rpms.sh" --srpm "$WORK/core"
    cp "$WORK"/core/srpms/*.src.rpm "$OUT/srpms/"
    for f in "$WORK"/core/srpms/*.src.rpm; do rust+=("$OUT/srpms/$(basename "$f")"); done
fi
if wanted standalone; then
    for b in "$REPO"/standalone/*/build-rpm.sh; do
        [ -x "$b" ] || continue
        name=$(basename "$(dirname "$b")")
        "$b" --srpm "$WORK/standalone"
        f=$(ls -t "$WORK/standalone/srpms/$name"-[0-9]*.src.rpm | head -1)
        cp "$f" "$OUT/srpms/"
        rust+=("$OUT/srpms/$(basename "$f")")
    done
fi
[ ${#rust[@]} -eq 0 ] || stage_line $((last_stage + 1)) "${rust[@]}"

{
    echo "# Source RPMs in $OUT/srpms, made $(date '+%Y-%m-%d %H:%M')."
    echo "# Build each stage after the earlier ones have succeeded. On the COPR web"
    echo "# page: Builds > New Build > Upload, one file at a time. With copr-cli"
    echo "# (it waits for each build): copr-cli build OWNER/PROJECT FILE..."
    echo "# The five core packages require each other's exact version-release:"
    echo "# upload all five of one run."
    cat "$order.tmp"
} >"$order"
rm -f "$order.tmp"
echo
cat "$order"
