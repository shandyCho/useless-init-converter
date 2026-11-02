#!/bin/bash
# User mode (USER_MODE=yes, work_history parts 136-140), unprivileged: a
# private bus, an s6-svscan and the daemon as a user's service manager would
# run them, with no s6-rc database. Every unit goes through the dynamic
# engine. Checks targets, services, .wants links, PartOf=/Conflicts= as the
# GNOME session uses them, and stopping.
#
#   tests/user-sandbox.sh WORKDIR       # runs the checks, then stops everything
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
BIN=$TOP/target/debug
W=${1:?usage: tests/user-sandbox.sh WORKDIR}
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-user-test
U=$W/units
rm -rf "$W" "$R"
mkdir -p "$W" "$U" "$R/scan" "$R/live" "$R/run" "$W/state" "$W/logs"
ln -s "$R/scan" "$R/live/scandir"
W=$(cd "$W" && pwd)
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
B="busctl --address=unix:path=$R/bus"
MGR="org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager"
upath() { $B call $MGR GetUnit s "$1" 2>/dev/null | cut -d'"' -f2; }
state() {
    local p
    p=$(upath "$1")
    [ -n "$p" ] || { echo "inactive/dead"; return; }
    echo "$($B get-property org.freedesktop.systemd1 "$p" org.freedesktop.systemd1.Unit ActiveState | cut -d'"' -f2)/$($B get-property org.freedesktop.systemd1 "$p" org.freedesktop.systemd1.Unit SubState | cut -d'"' -f2)"
}
expect_state() {
    local t=25 s
    while [ $t -gt 0 ]; do s=$(state "$1"); [ "$s" = "$2" ] && break; t=$((t - 1)); sleep 0.2; done
    [ "$s" = "$2" ] && ok "$1 is $2" || bad "$1: expected $2, got $s"
}
call() { $B call $MGR "$@" >/dev/null 2>"$W/last-error" || { cat "$W/last-error"; return 1; }; }
cleanup() {
    # shellcheck disable=SC2046 # one pid per word
    [ -s "$W/pids" ] && kill $(cat "$W/pids") 2>/dev/null
    # s6-svscan's children (supervisors, services) go with -t.
    [ -d "$R/scan/.s6-svscan" ] && s6-svscanctl -t "$R/scan" 2>/dev/null
    sleep 0.5
    true
}
trap cleanup EXIT

cat >"$W/uinitconverter.conf" <<EOF
USER_MODE=yes
UNIT_PATH=$U
RUNTIME_UNIT_DIR=$R/units
PRESET_PATH=
EXTRA_SOURCE=
STATE_DIR=$W/state
REPO_DIR=$W/repo
LIVE_DIR=$R/live
RUN_DIR=$R/run
LOG_DIR=$W/logs
BOOTED_FLAG=
PRIVATE_SOCKET=$R/private
BUS_ADDRESS=unix:path=$R/bus
ALLOW_UIDS=$(id -u)
RUNNER=$BIN/uinitconverter-runner
TIMER_RUNNER=$BIN/uinitconverter-timers
JOURNAL=no
TRANSIENT_DIR=$R/transient
UDEV_MONITOR=no
EXCLUDE=
EOF

# default.target wants base.service through a .wants link and a target.
cat >"$U/default.target" <<EOF
[Unit]
Wants=basic.target
EOF
cat >"$U/basic.target" <<EOF
[Unit]
EOF
mkdir -p "$U/default.target.wants"
cat >"$U/base.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
ln -s ../base.service "$U/default.target.wants/base.service"
# The GNOME session's shape, small: a session target that requires a
# manager service, a part that is PartOf= the session, and a shutdown
# target that conflicts with the session.
cat >"$U/sess@.target" <<EOF
[Unit]
Requires=sessmgr@%i.service
Wants=sesspart.service
Conflicts=sessdown.target
EOF
cat >"$U/sessmgr@.service" <<EOF
[Unit]
PartOf=sess@%i.target
[Service]
ExecStart=/bin/sleep 1000
EOF
cat >"$U/sesspart.service" <<EOF
[Unit]
PartOf=sess@gnome.target
[Service]
ExecStart=/bin/sleep 1000
EOF
cat >"$U/sessdown.target" <<EOF
[Unit]
Conflicts=sess@gnome.target
StopWhenUnneeded=yes
EOF

cat >"$W/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$R/bus</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
dbus-daemon --config-file="$W/bus.conf" --nofork --nopidfile >"$W/dbus.log" 2>&1 &
echo $! >>"$W/pids"
s6-svscan "$R/scan" >"$W/svscan.log" 2>&1 &
echo $! >>"$W/pids"
sleep 0.5
"$BIN/uinitconverterd" --config "$W/uinitconverter.conf" daemon --address "unix:path=$R/bus" >"$W/daemon.log" 2>&1 &
echo $! >>"$W/pids"

echo "== boot"
expect_state default.target active/active
expect_state basic.target active/active
expect_state base.service active/running
grep -q "default.target done" "$W/daemon.log" && ok "the daemon started default.target itself" || bad "daemon.log: $(tail -3 "$W/daemon.log")"

echo "== a session"
call StartUnit ss sess@gnome.target fail
expect_state sess@gnome.target active/active
expect_state sessmgr@gnome.service active/running
expect_state sesspart.service active/running

echo "== logout: the shutdown target conflicts with the session"
call StartUnit ss sessdown.target replace-irreversibly
expect_state sess@gnome.target inactive/dead
expect_state sessmgr@gnome.service inactive/dead
expect_state sesspart.service inactive/dead
expect_state sessdown.target inactive/dead
expect_state base.service active/running

echo "== a second session"
call StartUnit ss sess@gnome.target fail
expect_state sessmgr@gnome.service active/running

echo "== stopping a target stops its PartOf= units"
call StopUnit ss sess@gnome.target replace
expect_state sessmgr@gnome.service inactive/dead
expect_state sesspart.service inactive/dead

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
