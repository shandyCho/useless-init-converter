#!/bin/bash
# The unprivileged tests in a clean rootless podman container.
#
#   tests/container.sh build         # the image; once, and after the Containerfile changes
#   tests/container.sh run [ARGS]    # tests/all.sh --inner ARGS on a copy of this tree
#   tests/container.sh shell         # a shell there, for a failure
#   tests/container.sh clean         # remove the image and the volume
#
# Each step's log is in the volume, /work/logs: read it with `shell`.
# The tree is copied into a volume, which also keeps the crates and the build
# output, so this doesn't touch the host's target/ and uncommitted changes are
# tested too. Inside, the tests run as your user, as on the host. --privileged
# because the suite makes namespaces and mounts of its own, which a locked
# read-only /sys and /proc masking refuse. Rootless, that gives no more power over
# the host than your user already has; the container is for a clean set of
# packages and a separate PID namespace, not a security boundary.
#
# Needs podman and the skarnet RPMs: dist/rpm/build-skarnet-rpms.sh
# (SKARNET_RPMS=DIR if they aren't in ~/rpmbuild-skarnet/rpms).
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=localhost/uinitconverter-test
VOLUME=uinitconverter-test
FEDORA=$(rpm -E %fedora 2>/dev/null || echo 45)

usage() {
    sed -n '2,18s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

build() {
    local rpms=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/rpms} n=0 f
    ctx=$(mktemp -d)  # not local: the trap runs after this returns
    trap 'rm -rf "$ctx"' EXIT
    mkdir "$ctx/skarnet"
    for f in "$rpms"/*.rpm; do
        [ -e "$f" ] || continue
        case ${f##*/} in
            *-debuginfo-*|*-debugsource-*|*.src.rpm) continue ;;
        esac
        cp "$f" "$ctx/skarnet/"
        n=$((n + 1))
    done
    if [ "$n" = 0 ]; then
        echo "no skarnet RPMs in $rpms; run dist/rpm/build-skarnet-rpms.sh" >&2
        exit 1
    fi
    podman build --build-arg "FEDORA=$FEDORA" -t "$IMAGE" -f "$TOP/tests/Containerfile" "$ctx"
}

# In the container: bring the volume's copy up to date.
SYNC='mkdir -p /work/src /work/home && rm -rf /work/logs
rsync -a --delete --exclude=/target --exclude="/standalone/*/target" --exclude=/.git /src/ /work/src/
cd /work/src'

in_container() {
    podman image exists "$IMAGE" || build
    local tty=()
    [ -t 0 ] && [ -t 1 ] && tty=(-it)
    # XDG_RUNTIME_DIR not under /tmp: the suite's RUN_DIR is there, and
    # PrivateTmp='s bind on /tmp would hide its private directories.
    # No --init: its read-only bind at /run/podman-init can't be made writable
    # from a user namespace, which fails the sandbox's recursive /run. Host cgroup
    # paths, as memtest-check needs a process outside the namespace's root.
    podman run --rm "${tty[@]}" \
        --userns=keep-id \
        --cgroupns=host \
        --privileged \
        --pids-limit=-1 \
        --mount "type=bind,src=$TOP,dst=/src,ro" \
        --volume "$VOLUME:/work:U" \
        --tmpfs /tmp:rw,mode=1777 \
        --tmpfs /xdg:rw,mode=1777 \
        -e HOME=/work/home \
        -e CARGO_HOME=/work/home/.cargo \
        -e XDG_RUNTIME_DIR=/xdg \
        -e LOGDIR=/work/logs \
        "$IMAGE" "$@"
}

case ${1:-} in
    build) build ;;
    run)
        shift
        # shellcheck disable=SC2016  # expanded in there
        in_container bash -c "$SYNC"'
            exec tests/all.sh --inner "$@"' bash "$@"
        ;;
    shell)
        # shellcheck disable=SC2016  # expanded in there
        in_container bash -c "$SYNC"'
            exec bash'
        ;;
    clean)
        podman rmi -f "$IMAGE" 2>/dev/null || true
        podman volume rm -f "$VOLUME" 2>/dev/null || true
        ;;
    *) usage ;;
esac
