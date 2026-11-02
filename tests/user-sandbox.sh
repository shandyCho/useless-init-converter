#!/bin/bash
# The user manager (`uinitconverterd user-manager`, work_history parts
# 136-141), unprivileged: the launcher as user@UID.service runs it, with
# XDG_RUNTIME_DIR in a sandbox directory. It makes its own s6-svscan and
# configuration, serves the private socket at once, starts the user bus
# itself (dbus.socket, then dbus.service on the first client) and goes on
# the bus. Checks targets, services, .wants links, PartOf=/Conflicts= as the
# GNOME session uses them, a bus restart (gnome-session-ctl --restart-dbus)
# and the stop on SIGTERM. Every unit goes through the dynamic engine.
#
#   tests/user-sandbox.sh WORKDIR       # runs the checks, then stops everything
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
BIN=$TOP/target/debug
W=${1:?usage: tests/user-sandbox.sh WORKDIR}
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-user-test
U=$W/units
rm -rf "$W" "$R"
mkdir -p "$W" "$U" "$R" "$W/home"
W=$(cd "$W" && pwd)
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
B="busctl --address=unix:path=$R/bus"
MGR="org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager"
# systemctl --user finds the manager at $XDG_RUNTIME_DIR/systemd/private;
# the bus address points into the sandbox too, so it never reaches the
# host's own user manager.
SC() { env XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" systemctl --user "$@"; }
state() {
    echo "$(SC show -p ActiveState --value "$1" 2>/dev/null)/$(SC show -p SubState --value "$1" 2>/dev/null)"
}
expect_state() {
    local t=25 s
    while [ $t -gt 0 ]; do s=$(state "$1"); [ "$s" = "$2" ] && break; t=$((t - 1)); sleep 0.2; done
    [ "$s" = "$2" ] && ok "$1 is $2" || bad "$1: expected $2, got $s"
}
waitfor() {
    local t=50
    while [ $t -gt 0 ]; do eval "$1" && return 0; t=$((t - 1)); sleep 0.2; done
    return 1
}
# Through the bus, as GNOME's callers do.
call() { $B call $MGR "$@" >/dev/null 2>"$W/last-error" || { cat "$W/last-error"; return 1; }; }
cleanup() {
    # shellcheck disable=SC2046 # one pid per word
    [ -s "$W/pids" ] && kill $(cat "$W/pids") 2>/dev/null
    sleep 1
    # s6-svscan's children (supervisors, services) go with -t, if the
    # launcher did not end it.
    [ -d "$R/uinitconverter/scan/.s6-svscan" ] && s6-svscanctl -t "$R/uinitconverter/scan" 2>/dev/null
    sleep 0.5
    true
}
trap cleanup EXIT

# The system configuration the launcher reads: tools and logging. It
# writes the user manager's own under $R/uinitconverter.
cat >"$W/system.conf" <<EOF
USER_MANAGER=s6
RUNNER=$BIN/uinitconverter-runner
TIMER_RUNNER=$BIN/uinitconverter-timers
JOURNAL=no
EOF

# default.target wants base.service through a .wants link and a target,
# and the bus through sockets.target, as on Fedora.
cat >"$U/default.target" <<EOF
[Unit]
Wants=basic.target sockets.target
EOF
cat >"$U/basic.target" <<EOF
[Unit]
EOF
cat >"$U/sockets.target" <<EOF
[Unit]
EOF
mkdir -p "$U/default.target.wants" "$U/sockets.target.wants"
cat >"$U/base.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
ln -s ../base.service "$U/default.target.wants/base.service"
cat >"$U/dbus.socket" <<EOF
[Socket]
ListenStream=%t/bus
EOF
ln -s ../dbus.socket "$U/sockets.target.wants/dbus.socket"
cat >"$U/dbus-test.service" <<EOF
[Unit]
Requires=dbus.socket
After=dbus.socket
[Service]
Sockets=dbus.socket
ExecStart=/usr/bin/dbus-daemon --config-file=$W/bus.conf --nofork --nopidfile
EOF
ln -s dbus-test.service "$U/dbus.service"
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

# Slice B: an environment generator, a unit that reads the manager's
# environment, a Type=dbus unit (its name on the user bus) and a
# Type=notify one that says READY=1 itself.
mkdir -p "$W/generators"
cat >"$W/generators/50-test" <<EOF
#!/bin/sh
echo "GEN_SEEN=\$HOME"
echo "FROM_GENERATOR=yes"
EOF
chmod +x "$W/generators/50-test"
cat >"$U/envdump.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo "\$FROM_GENERATOR \$LATER" >$W/envdump.out'
EOF
cat >"$W/ownname.py" <<'EOF'
from gi.repository import Gio, GLib
Gio.bus_own_name(Gio.BusType.SESSION, "org.example.UserTest", 0, None, None, None)
GLib.MainLoop().run()
EOF
cat >"$W/notifier.py" <<'EOF'
import os, socket, time
time.sleep(1)
a = os.environ["NOTIFY_SOCKET"]
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.connect("\0" + a[1:] if a.startswith("@") else a)
s.send(b"READY=1")
time.sleep(1000)
EOF
cat >"$U/busname.service" <<EOF
[Service]
Type=dbus
BusName=org.example.UserTest
ExecStart=/usr/bin/python3 $W/ownname.py
EOF
cat >"$U/notifier.service" <<EOF
[Service]
Type=notify
ExecStart=/usr/bin/python3 $W/notifier.py
EOF

# gnome-session@gnome-login.target's shape: the login target requires the
# initialized target and the shell; initialized requires pre, which is
# Requisite= and Before= of it; the shell is Requisite= and Before= of
# initialized, so it starts first within the transaction; initialized is
# Before= the signal-init oneshot. A template named without an instance
# takes the login target's.
cat >"$U/login@.target" <<EOF
[Unit]
Requires=initd.target
Requires=shellx@%i.service
Requires=lmgr@.service
EOF
cat >"$U/initd.target" <<EOF
[Unit]
Requires=pre.target
After=pre.target
Requires=siginit.service
Before=siginit.service
EOF
cat >"$U/pre.target" <<EOF
[Unit]
Requisite=initd.target
Before=initd.target
EOF
cat >"$U/shellx@.service" <<EOF
[Unit]
Requisite=initd.target
Before=initd.target
[Service]
ExecStart=/bin/sh -c 'echo shell >>$W/order.out; exec sleep 1000'
EOF
cat >"$U/siginit.service" <<EOF
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'sleep 0.5; echo signal >>$W/order.out'
EOF
cat >"$U/lmgr@.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF

# xdg-desktop-portal's case: Requisite= and After= a target that another
# transaction is still bringing up.
cat >"$U/slowpre.service" <<EOF
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sleep 2
EOF
cat >"$U/late.target" <<EOF
[Unit]
Requires=slowpre.service
After=slowpre.service
EOF
cat >"$U/outer.target" <<EOF
[Unit]
Requires=late.target
EOF
cat >"$U/portalx.service" <<EOF
[Unit]
Requisite=late.target
After=late.target
[Service]
ExecStart=/bin/sleep 1000
EOF

cat >"$W/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>systemd:</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

# READY=1 from the launcher arrives here.
python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.bind(sys.argv[1])
open(sys.argv[2], "a").write(s.recv(4096).decode() + "\n")
' "$R/notify" "$W/notified" &
echo $! >>"$W/pids"
sleep 0.3
env HOME="$W/home" XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" \
    NOTIFY_SOCKET="$R/notify" UINITCONVERTER_USER_UNIT_PATH="$U" UINITCONVERTER_USER_CGROUP=no \
    UINITCONVERTER_USER_GENERATOR_PATH="$W/generators" \
    "$BIN/uinitconverterd" --config "$W/system.conf" user-manager >"$W/daemon.log" 2>&1 &
MGR_PID=$!
echo $MGR_PID >>"$W/pids"

echo "== boot"
waitfor '[ -S "$R/systemd/private" ]' && ok "the private socket is served" || bad "no $R/systemd/private: $(tail -3 "$W/daemon.log")"
waitfor 'grep -q "default.target done" "$W/daemon.log"' && ok "the daemon started default.target itself" || bad "daemon.log: $(tail -3 "$W/daemon.log")"
waitfor 'grep -q READY=1 "$W/notified" 2>/dev/null' && ok "READY=1 sent to user@" || bad "no READY=1"
waitfor 'grep -q "on the bus" "$W/daemon.log"' && ok "the daemon is on the bus it started" || bad "daemon.log: $(tail -3 "$W/daemon.log")"
expect_state default.target active/active
expect_state basic.target active/active
expect_state base.service active/running
expect_state dbus.socket active/running
expect_state dbus.service active/running
out=$($B call $MGR GetUnit s base.service 2>&1) && ok "GetUnit over the user bus" || bad "GetUnit on the bus: $out"
out=$(SC show-environment 2>&1)
case $out in *"DBUS_SESSION_BUS_ADDRESS=unix:path=$R/bus"*) ok "the units get the bus address" ;; *) bad "manager environment: $out" ;; esac

case $out in *"FROM_GENERATOR=yes"*"GEN_SEEN=$W/home"*|*"GEN_SEEN=$W/home"*"FROM_GENERATOR=yes"*) ok "the environment generator's output (it saw HOME)" ;; *) bad "generator: $out" ;; esac

echo "== slice B: environment, Type=dbus, Type=notify"
SC set-environment LATER=set-later
SC start envdump.service
[ "$(cat "$W/envdump.out" 2>/dev/null)" = "yes set-later" ] && ok "a unit sees the generator's and SetEnvironment's variables" \
    || bad "envdump: $(cat "$W/envdump.out" 2>&1)"
t0=$(date +%s)
if SC start busname.service; then
    out=$($B status org.example.UserTest 2>/dev/null | sed -n 's/^PID=//p')
    [ -n "$out" ] && [ "$out" = "$(SC show -p MainPID --value busname.service)" ] \
        && ok "Type=dbus: started once its name was on the user bus" || bad "BusName owner ${out:-none}"
else
    bad "busname.service did not start: $(SC status busname.service 2>&1 | tail -3)"
fi
SC start notifier.service && [ $(($(date +%s) - t0)) -ge 1 ] && expect_state notifier.service active/running \
    || bad "notifier.service: $(SC status notifier.service 2>&1 | tail -3)"

echo "== a transaction: Requisite=, Before=/After=, a template without instance"
call StartUnit ss login@gdm.target fail && ok "login@gdm.target started" || bad "login@gdm.target: $(cat "$W/last-error")"
expect_state initd.target active/active
expect_state pre.target active/active
expect_state shellx@gdm.service active/running
expect_state lmgr@gdm.service active/running
waitfor '[ "$(wc -l <"$W/order.out" 2>/dev/null)" -ge 2 ]' || true
[ "$(tr '\n' ' ' <"$W/order.out" 2>/dev/null)" = "shell signal " ] && ok "the shell started before initd.target's signal" \
    || bad "order: $(cat "$W/order.out" 2>&1)"
SC stop login@gdm.target initd.target pre.target shellx@gdm.service lmgr@gdm.service siginit.service
if SC start pre.target 2>/dev/null; then bad "pre.target started though its requisite is down"; else ok "Requisite=: pre.target refused while initd.target is down"; fi
expect_state initd.target inactive/dead

call StartUnit ss outer.target replace
sleep 0.3
SC start portalx.service && ok "Requisite= waited for the target another transaction was starting" \
    || bad "portalx.service: $(SC status portalx.service 2>&1 | tail -2)"
expect_state portalx.service active/running

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

echo "== the bus restarts (gnome-session-ctl --restart-dbus stops it)"
old=$(SC show -p MainPID --value dbus.service)
SC stop dbus.service || true
waitfor 'grep -q "the bus went away" "$W/daemon.log"' && ok "the daemon saw the bus go" || bad "daemon.log: $(tail -3 "$W/daemon.log")"
out=$(SC is-active base.service 2>&1) || true
[ "$out" = active ] && ok "systemctl --user works without the bus" || bad "systemctl --user without the bus: $out"
waitfor '[ "$(grep -c "on the bus" "$W/daemon.log")" -ge 2 ]' && ok "back on the bus: its socket started it again" || bad "daemon.log: $(tail -5 "$W/daemon.log")"
new=$(SC show -p MainPID --value dbus.service)
[ -n "$new" ] && [ "$new" != 0 ] && [ "$new" != "$old" ] && ok "a new bus ($old -> $new)" || bad "dbus.service MainPID $old -> $new"
out=$($B call $MGR GetUnit s base.service 2>&1) && ok "GetUnit over the new bus" || bad "GetUnit on the new bus: $out"

echo "== SIGTERM (user@ stops): every unit stops, then s6-svscan"
base_pid=$(SC show -p MainPID --value base.service)
kill -TERM "$MGR_PID"
waitfor '! kill -0 "$MGR_PID" 2>/dev/null' && ok "the user manager exited" || bad "the user manager still runs"
if kill -0 "$base_pid" 2>/dev/null; then bad "base.service PID $base_pid still runs"; else ok "base.service's process is gone"; fi
if s6-svscanctl -z "$R/uinitconverter/scan" 2>/dev/null; then bad "s6-svscan still runs"; else ok "s6-svscan ended"; fi
if [ "$(grep -c "stopped, its socket listens again" "$W/daemon.log")" -gt 1 ]; then
    bad "the bus was armed again while exiting"
else
    ok "the bus was not armed again while exiting"
fi

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
