#!/bin/sh
# Run a real systemctl against the sandbox from tests/sandbox.sh.
#
# It runs inside an unprivileged user+mount namespace with a private /run that
# contains /run/systemd/system (the sd_booted() flag) and /run/systemd/private
# bound to the sandbox daemon's socket, so the host's own systemd (if any) is
# never contacted.
#
#   tests/systemctl.sh status hello.service           # host systemctl
#   SYSTEMD_ROOT=/path/to/extracted-rpm tests/systemctl.sh list-units
set -eu

RU=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
SB=$RU/sdc-test
ROOT=${SYSTEMD_ROOT:-}
if [ -n "$ROOT" ]; then
    SYSTEMCTL=${SYSTEMCTL:-$ROOT/usr/bin/systemctl}
    LIBS=$ROOT/usr/lib64/systemd:$ROOT/usr/lib64
else
    SYSTEMCTL=${SYSTEMCTL:-$(command -v systemctl)}
    LIBS=${LD_LIBRARY_PATH:-}
fi
[ -S "$SB/private" ] || { echo "sandbox not running (tests/sandbox.sh)" >&2; exit 1; }

BIND=${TMPDIR:-/tmp}/sdcompat-ns-$(id -u)
mkdir -p "$BIND"

exec unshare -rm --propagation private sh -c '
    ru=$1 bind=$2 sb=$3 sysctl=$4 libs=$5
    shift 5
    mount --rbind "$ru" "$bind"
    mount -t tmpfs none /run
    mkdir -p /run/systemd/system "$ru"
    touch /run/systemd/private
    mount --rbind "$bind" "$ru"
    mount --bind "$sb/private" /run/systemd/private
    export SYSTEMD_IGNORE_CHROOT=1 SYSTEMD_PAGER=cat SYSTEMD_COLORS=0
    export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$sb/bus"
    [ -n "$libs" ] && export LD_LIBRARY_PATH="$libs"
    exec "$sysctl" "$@"
' sh "$RU" "$BIND" "$SB" "$SYSTEMCTL" "$LIBS" "$@"
