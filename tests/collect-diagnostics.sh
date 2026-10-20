#!/bin/bash
# Collect what is needed to debug sdcompat on a test machine into one tarball.
#
#   sudo tests/collect-diagnostics.sh            # after booting the s6 entry
#   tests/collect-diagnostics.sh ~/sdc           # after a failed sandbox run
#
# It only reads; nothing is changed. Review the archive before sharing it:
# it contains unit names, log lines and kernel/boot settings of this machine.
set -u

SANDBOX=${1:-}
REPO=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
OUT=$(mktemp -d /tmp/sdcompat-diag.XXXXXX)
D=$OUT/diag
mkdir -p "$D"

run() { # run NAME CMD...
    local name=$1
    shift
    { echo "\$ $*"; timeout 30 "$@" 2>&1; echo "[exit $?]"; } >"$D/$name.txt"
}
copy() { [ -e "$1" ] && cp -a "$1" "$D/$2" 2>/dev/null; }

run version git -C "$REPO" log -1 --oneline
run os cat /etc/os-release
run kernel uname -a
run cmdline cat /proc/cmdline
run pid1 cat /proc/1/comm
run selinux getenforce

if [ -n "$SANDBOX" ]; then
    R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-test
    copy "$SANDBOX/daemon.log" daemon.log
    copy "$SANDBOX/svscan.log" svscan.log
    copy "$SANDBOX/dbus.log" dbus.log
    copy "$SANDBOX/sdcompat.conf" sdcompat.conf
    copy "$SANDBOX/state/current/report.txt" report.txt
    run s6rc-active s6-rc -l "$R/live" -a list
    copy "$SANDBOX/check.out" check.out
else
    run s6rc-active s6-rc -a list
    run s6rc-all s6-rc-db list all
    run units systemctl list-units --all --no-pager
    run failed systemctl --failed --no-pager
    run timers systemctl list-timers --all --no-pager
    run system-state systemctl is-system-running
    run logind-status systemctl status systemd-logind --no-pager
    run journal-boot journalctl -b --no-pager -n 3000
    run journal-errors journalctl -b -p warning --no-pager -n 1000
    run sessions loginctl list-sessions --no-pager
    run cgroups systemd-cgls --no-pager
    copy /run/uncaught-logs/current uncaught-logs.txt
    copy /var/lib/sdcompat/current/report.txt report.txt
    copy /etc/sdcompat/sdcompat.conf sdcompat.conf
    copy /etc/sdcompat/boot-options boot-options
    # setup-fedora copies these root-owned; dbus's socket needs its own label.
    run s6-services ls -lnZ /etc/sdcompat/s6
    run dbus-socket ls -lnZ /run/dbus/system_bus_socket
    run earlyoom s6-svstat /run/service/earlyoom
    copy /run/sdcompat/devices devices.txt
    for f in /boot/loader/entries/*-sdcompat.conf; do copy "$f" "boot-entry-$(basename "$f")"; done
    run avc ausearch -m AVC,USER_AVC -ts boot
fi

tar -C "$OUT" -czf "$OUT.tar.gz" diag
rm -rf "$OUT"
echo "Diagnostics written to $OUT.tar.gz"
echo "Check its contents before sharing:  tar -tzf $OUT.tar.gz"
