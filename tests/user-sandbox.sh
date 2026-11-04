#!/bin/bash
# The user manager (`initconvert-user-manager`, a link to initconvertd as
# the uinitconverter-user package installs it; work_history parts
# 136-141), unprivileged: the launcher as user@UID.service runs it, with
# XDG_RUNTIME_DIR in a sandbox directory. It makes its own s6-svscan and
# configuration, serves the private socket at once, starts the user bus
# itself (dbus.socket, then dbus.service on the first client) and goes on
# the bus. Checks targets, services, .wants links, PartOf=/Conflicts= as the
# GNOME session uses them, a bus restart (gnome-session-ctl --restart-dbus)
# and the stop on SIGTERM. Every unit goes through the dynamic engine.
# Slices C-E: unit generators, timers and paths, Accept=yes, MAINPID=,
# job merging, stop transactions, CollectMode=, systemctl --user exit and
# the fallback to systemd --user (a fake one records that it ran).
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
    # Some have ended already (the last manager is killed and reaped before
    # this): a failed kill must not end the trap under set -e, which made the
    # script exit 1 after "failed: 0".
    [ -s "$W/pids" ] && kill $(cat "$W/pids") 2>/dev/null || true
    sleep 1
    # s6-svscan's children (supervisors, services) go with -t, if the
    # launcher did not end it.
    [ -d "$R/uinitconverter/scan/.s6-svscan" ] && s6-svscanctl -t "$R/uinitconverter/scan" 2>/dev/null || true
    sleep 0.5
    true
}
trap cleanup EXIT

# The system configuration the launcher reads: tools and logging. It
# writes the user manager's own under $R/uinitconverter.
cat >"$W/system.conf" <<EOF
USER_MANAGER=s6
USER_FALLBACK_ON=fbdown.target fbsess@*.target
RUNNER=$BIN/initconvert-runner
TIMER_RUNNER=$BIN/initconvert-timers
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
Wants=sesspart.service sessreq.service sessmon.service
Conflicts=sessdown.target
EOF
# gnome-session-monitor.service's shape: PartOf= a unit that Requires= the
# session (stopped with it, so its PartOf= unit must go too).
cat >"$U/sessreq.service" <<EOF
[Unit]
Requires=sess@gnome.target
[Service]
ExecStart=/bin/sleep 1000
EOF
cat >"$U/sessmon.service" <<EOF
[Unit]
PartOf=sessreq.service
[Service]
ExecStart=/bin/sleep 1000
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
# A main process that waits for its child on SIGTERM (gnome-shell and its
# glycin loaders): KillMode=control-group signals both at the stop's start.
cat >"$U/waitkid.service" <<'EOF'
[Service]
ExecStart=/bin/sh -c 'trap "wait; exit 0" TERM; sleep 1000 & wait'
TimeoutStopSec=8
EOF
# wireplumber's shape: BindsTo= a service it exits 0 without (pipewire). A
# restart of the host restarts it too, as systemd's restart job does.
cat >"$U/bindhost.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'echo \$\$\$\$ >$W/bindhost.pid; exec sleep 1000'
EOF
cat >"$U/bound.service" <<EOF
[Unit]
BindsTo=bindhost.service
After=bindhost.service
[Service]
ExecStart=/bin/sh -c 'p=\$\$(cat $W/bindhost.pid); while kill -0 \$\$p 2>/dev/null; do sleep 0.1; done'
EOF
# Job modes: slowstart's start takes 3 s, so jobs queue behind it.
cat >"$U/slowstart.service" <<EOF
[Service]
ExecStartPre=/bin/sleep 3
ExecStart=/bin/sleep 1000
EOF
cat >"$U/needer.service" <<EOF
[Unit]
Requires=needed.service
After=needed.service
[Service]
ExecStart=/bin/sleep 1000
EOF
cat >"$U/needed.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
cat >"$U/quick.service" <<EOF
[Service]
ExecStart=/bin/true
EOF
# Type=dbus units that exit before taking their name (evolution-alarm-notify
# --autostart with nothing to do): exit 0 is a done job, as in systemd.
cat >"$U/busquit.service" <<EOF
[Service]
Type=dbus
BusName=org.example.NeverTaken
ExecStart=/bin/true
EOF
cat >"$U/busfail.service" <<EOF
[Service]
Type=dbus
BusName=org.example.NeverTaken
ExecStart=/bin/false
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

# Slice C: a unit generator, as systemd-xdg-autostart-generator: it writes
# a unit and a .wants link into its third directory (generator.late). It
# names the unit after $W/genname, so a reload shows it ran again.
mkdir -p "$W/unitgens"
echo first >"$W/genname"
cat >"$W/unitgens/test-generator" <<EOF
#!/bin/sh
[ "\$SYSTEMD_SCOPE" = user ] || exit 1
n=\$(cat $W/genname)
mkdir -p "\$3/autostartx.target.wants"
printf '[Service]\nExecStart=/bin/sleep 1000\n' >"\$3/app-\$n@autostart.service"
ln -s "../app-\$n@autostart.service" "\$3/autostartx.target.wants/app-\$n@autostart.service"
echo "\$1 \$2" >$W/genargs
EOF
chmod +x "$W/unitgens/test-generator"
cat >"$U/autostartx.target" <<EOF
[Unit]
EOF

# Slice D: a timer and a path unit, wanted by timers.target/paths.target
# from default.target as on Fedora; a Persistent= timer, whose stamp goes
# where systemd --user keeps it.
cat >>"$U/default.target" <<EOF
Wants=timers.target paths.target
EOF
printf '[Unit]\n' >"$U/timers.target"
printf '[Unit]\n' >"$U/paths.target"
mkdir -p "$U/timers.target.wants" "$U/paths.target.wants"
cat >"$U/tick.timer" <<EOF
[Timer]
OnActiveSec=1
OnUnitActiveSec=1
AccuracySec=1ms
EOF
cat >"$U/tick.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo tick >>$W/ticks'
EOF
ln -s ../tick.timer "$U/timers.target.wants/tick.timer"
cat >"$U/daily.timer" <<EOF
[Timer]
OnCalendar=daily
Persistent=yes
EOF
cat >"$U/daily.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo daily >>$W/daily'
EOF
ln -s ../daily.timer "$U/timers.target.wants/daily.timer"
cat >"$U/watch.path" <<EOF
[Path]
PathExists=$W/trigger
EOF
cat >"$U/watch.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo seen >$W/watched; rm -f $W/trigger'
EOF
ln -s ../watch.path "$U/paths.target.wants/watch.path"
# An Accept=yes socket (systemd's varlink sockets in sockets.target are):
# one service instance per connection.
cat >"$U/echoer.socket" <<EOF
[Socket]
ListenStream=%t/echoer
Accept=yes
EOF
cat >"$U/echoer@.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'read -r l; echo "got \$\$l"'
StandardInput=socket
EOF
ln -s ../echoer.socket "$U/sockets.target.wants/echoer.socket"
# systemd's varlink services serve only on an fd named "varlink".
cat >"$U/named.socket" <<EOF
[Socket]
ListenStream=%t/named
Accept=yes
FileDescriptorName=varlink
EOF
cat >"$U/named@.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'echo "\$\$LISTEN_FDS \$\$LISTEN_FDNAMES" >$W/fdname'
EOF
ln -s ../named.socket "$U/sockets.target.wants/named.socket"
# pipewire's shape: a socket-activated service whose session manager comes
# from its .wants/ (/etc/systemd/user/pipewire.service.wants/wireplumber):
# the activation starts it, as systemd's start job for the service does.
cat >"$U/pw.socket" <<EOF
[Socket]
ListenStream=%t/pw
EOF
cat >"$U/pw.service" <<EOF
[Service]
ExecStart=/usr/bin/python3 -c 'import socket, time; s = socket.socket(fileno=3); c, _ = s.accept(); c.sendall(b"x"); time.sleep(1000)'
EOF
cat >"$U/plumb.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
mkdir -p "$U/pw.service.wants"
ln -s ../plumb.service "$U/pw.service.wants/plumb.service"
ln -s ../pw.socket "$U/sockets.target.wants/pw.socket"
# podman's quadlet shape: the start process forks the real one, which is
# announced with MAINPID= (NotifyAccess=all), then exits.
cat >"$W/handover.py" <<'EOF'
import os, socket, sys
pid = os.fork()
if pid == 0:
    os.setsid()
    os.execv("/bin/sleep", ["sleep", "1000"])
open(sys.argv[1], "w").write(str(pid))
a = os.environ["NOTIFY_SOCKET"]
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.connect("\0" + a[1:] if a.startswith("@") else a)
s.send(("MAINPID=%d\nREADY=1" % pid).encode())
EOF
cat >"$U/handover.service" <<EOF
[Service]
Type=notify
NotifyAccess=all
ExecStart=/usr/bin/python3 $W/handover.py $W/handover.pid
EOF

# Slice E: systemctl --user exit (exit.target, SuccessAction=exit-force,
# as systemd's), and the fallback's triggers: a unit failing into
# USER_FALLBACK_ON= (GNOME's OnFailure=gnome-session-shutdown.target), a
# failed start of a fallback target. systemd --user is a fake that
# records its PID and arguments.
cat >"$U/exit.target" <<EOF
[Unit]
Requires=systemd-exit.service
After=systemd-exit.service
EOF
cat >"$U/systemd-exit.service" <<EOF
[Unit]
SuccessAction=exit-force
EOF
cat >"$U/sessfail.service" <<EOF
[Unit]
OnFailure=fbdown.target
[Service]
ExecStart=/bin/sh -c 'sleep 0.5; exit 3'
EOF
printf '[Unit]\n' >"$U/fbdown.target"
cat >"$U/fbsess@.target" <<EOF
[Unit]
Requires=broken.service
After=broken.service
EOF
cat >"$U/broken.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/false
EOF
cat >"$W/fake-systemd" <<EOF
#!/bin/sh
echo "\$\$ \$*" >$W/fake-systemd.out
exec sleep 1000
EOF
chmod +x "$W/fake-systemd"
mkdir -p "$W/fb"
chmod 1777 "$W/fb"

# Parallel starts in a transaction: three unordered oneshots of 2 s, and
# one ordered after the first.
cat >"$U/par.target" <<EOF
[Unit]
Wants=par1.service par2.service par3.service parafter.service
EOF
for n in 1 2 3; do
    cat >"$U/par$n.service" <<EOF
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'sleep 2; date +%%s.%%N >$W/par$n.end'
EOF
done
cat >"$U/parafter.service" <<EOF
[Unit]
After=par1.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'date +%%s.%%N >$W/parafter.start'
EOF

# Job merging: gnome-session-shutdown.target's shape (StopWhenUnneeded=,
# a wanted oneshot), started three times at once.
cat >"$U/mergetgt.target" <<EOF
[Unit]
Wants=counter.service
Before=counter.service
StopWhenUnneeded=yes
EOF
cat >"$U/counter.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'sleep 3; echo run >>$W/counter'
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
# start_mgr LOG: the launcher, as user@UID.service runs it: by the name the
# uinitconverter-user package gives it (a link to initconvertd).
ln -sf "$BIN/initconvertd" "$W/initconvert-user-manager"
start_mgr() {
    env HOME="$W/home" XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" \
        NOTIFY_SOCKET="$R/notify" UINITCONVERTER_USER_UNIT_PATH="$U:$R/systemd/generator.late" UINITCONVERTER_USER_CGROUP=no \
        UINITCONVERTER_USER_GENERATOR_PATH="$W/generators" UINITCONVERTER_USER_UNIT_GENERATOR_PATH="$W/unitgens" \
        UINITCONVERTER_SYSTEMD_USER="$W/fake-systemd" UINITCONVERTER_USER_FALLBACK_DIR="$W/fb" \
        "$W/initconvert-user-manager" --config "$W/system.conf" >"$1" 2>&1 &
    MGR_PID=$!
    echo $MGR_PID >>"$W/pids"
}
start_mgr "$W/daemon.log"

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
if SC start busquit.service; then ok "Type=dbus exiting 0 before its name: the start job is done"; else bad "busquit.service's start failed"; fi
expect_state busquit.service inactive/dead
[ "$(SC show -p Result --value busquit.service)" = success ] && ok "busquit.service's result is success" \
    || bad "busquit.service: Result=$(SC show -p Result --value busquit.service)"
if SC start busfail.service 2>/dev/null; then bad "busfail.service's start succeeded"; else ok "Type=dbus exiting 1 before its name: the start fails"; fi
[ "$(SC show -p Result --value busfail.service)" = exit-code ] && ok "busfail.service's result is exit-code" \
    || bad "busfail.service: Result=$(SC show -p Result --value busfail.service)"
SC reset-failed busfail.service
SC start waitkid.service
sleep 0.5
k0=$(date +%s%N)
SC stop waitkid.service
ms=$((($(date +%s%N) - k0) / 1000000))
[ "$(state waitkid.service)" = inactive/dead ] && [ $ms -lt 3000 ] && ok "KillMode=control-group: the child got SIGTERM with the main process ($ms ms)" \
    || bad "waitkid.service: $(state waitkid.service) after $ms ms"
SC start bound.service
expect_state bound.service active/running
b1=$(SC show -p MainPID --value bound.service)
SC restart bindhost.service
expect_state bindhost.service active/running
expect_state bound.service active/running
b2=$(SC show -p MainPID --value bound.service)
[ -n "$b2" ] && [ "$b2" != 0 ] && [ "$b2" != "$b1" ] && ok "a BindsTo= dependent was restarted with its host ($b1 -> $b2)" || bad "bound.service: MainPID $b1 -> $b2"
SC stop bound.service bindhost.service
out=$(SC show -p ActiveState,Id,MainPID,Type busname.service | cut -d= -f1 | tr '\n' ' ')
[ "$out" = "Id ActiveState Type MainPID " ] && ok "show -p prints in systemd's order (Unit's, then Service's vtable order)" || bad "show -p order: $out"
echo "== job modes"
if call StartUnit ss slowstart.service bogus; then bad "an unknown job mode was accepted"; else
    grep -q "Job mode bogus invalid" "$W/last-error" && ok "an unknown job mode is refused" || bad "unknown mode: $(cat "$W/last-error")"; fi
SC start --no-block slowstart.service
sleep 0.5
if SC stop --job-mode=fail slowstart.service 2>"$W/jm.err"; then bad "fail: a stop beside the running start was accepted"; else
    grep -q "destructive" "$W/jm.err" && ok "fail: a stop beside the running start is refused" || bad "fail: $(cat "$W/jm.err")"; fi
expect_state slowstart.service active/running
SC stop slowstart.service
# A start running, a stop queued behind it, then a start: the stop is
# cancelled, so the first start's process stays (queued in turn, the
# stop and a new 3 s start would have replaced it by the second look).
SC start --no-block slowstart.service
sleep 0.5
SC stop --no-block slowstart.service
SC start --no-block slowstart.service
expect_state slowstart.service active/running
p1=$(SC show -p MainPID --value slowstart.service)
sleep 4.5
p2=$(SC show -p MainPID --value slowstart.service)
[ -n "$p1" ] && [ "$p1" != 0 ] && [ "$p1" = "$p2" ] && ok "replace: the queued stop was cancelled by the new start ($p1 kept)" || bad "replace: MainPID $p1 -> $p2"
SC stop slowstart.service
SC start --no-block --job-mode=replace-irreversibly slowstart.service
sleep 0.5
if SC stop slowstart.service 2>"$W/jm.err"; then bad "replace-irreversibly: a stop replaced it"; else
    grep -q "destructive" "$W/jm.err" && ok "replace-irreversibly: a conflicting stop is refused" || bad "irreversible: $(cat "$W/jm.err")"; fi
expect_state slowstart.service active/running
SC stop slowstart.service
SC start --job-mode=ignore-dependencies needer.service
expect_state needer.service active/running
expect_state needed.service inactive/dead
SC stop needer.service

# Type=simple exiting at once (user-dirs-update-gtk): its runner's exec
# helper may stamp it active only after it ended; still a done job.
n=0
for _ in 1 2 3 4 5 6 7 8 9 10; do SC start quick.service 2>/dev/null || n=$((n + 1)); done
[ "$n" -eq 0 ] && ok "Type=simple exiting 0 at once: 10 start jobs done" || bad "Type=simple exiting 0 at once: $n of 10 starts failed"
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

echo "== slice C: unit generators"
[ "$(cat "$W/genargs" 2>/dev/null)" = "$R/systemd/generator $R/systemd/generator.early" ] \
    && ok "the unit generator ran with the three directories" || bad "genargs: $(cat "$W/genargs" 2>&1)"
SC start autostartx.target
expect_state app-first@autostart.service active/running
echo second >"$W/genname"
SC stop autostartx.target app-first@autostart.service
SC daemon-reload
SC start autostartx.target
expect_state app-second@autostart.service active/running
out=$(SC show -p LoadState --value app-first@autostart.service 2>&1)
[ "$out" = not-found ] && ok "after a reload the generator's old output is gone" || bad "app-first@autostart.service: $out"
SC stop autostartx.target app-second@autostart.service

echo "== slice D: timers and paths"
expect_state timers.target active/active
expect_state tick.timer active/waiting
waitfor '[ "$(wc -l <"$W/ticks" 2>/dev/null)" -ge 2 ]' && ok "tick.timer ran tick.service more than once" || bad "ticks: $(cat "$W/ticks" 2>&1)"
out=$(SC list-timers --all --no-legend 2>&1)
case $out in *tick.timer*daily.timer* | *daily.timer*tick.timer*) ok "list-timers shows both timers" ;; *) bad "list-timers: $out" ;; esac
expect_state daily.timer active/waiting
# The first start of a Persistent= timer with no stamp does not catch up;
# a stamp from two days ago does (after a restart of the timer).
mkdir -p "$W/home/.local/share/systemd/timers"
touch -d '2 days ago' "$W/home/.local/share/systemd/timers/stamp-daily.timer"
SC restart daily.timer
waitfor '[ -s "$W/daily" ]' && ok "Persistent=: a missed elapse was caught up" || bad "daily.service did not run: $(SC status daily.timer 2>&1 | tail -3)"
waitfor '[ "$(stat -c %Y "$W/home/.local/share/systemd/timers/stamp-daily.timer")" -gt "$(date -d "1 day ago" +%s)" ]' \
    && ok "the stamp is in ~/.local/share/systemd/timers, as systemd --user keeps it" || bad "stamp: $(ls -la "$W/home/.local/share/systemd/timers" 2>&1)"
expect_state watch.path active/waiting
touch "$W/trigger"
waitfor '[ -s "$W/watched" ]' && ok "watch.path started its service" || bad "watch.path: $(SC status watch.path 2>&1 | tail -3)"

expect_state echoer.socket active/listening
out=$(echo hello | timeout 5 python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall(b"hello\n"); s.shutdown(socket.SHUT_WR)
print(s.recv(100).decode().strip())' "$R/echoer" 2>&1)
[ "$out" = "got hello" ] && ok "Accept=yes: a connection got its own instance" || bad "echoer: $out"
python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.recv(1)' "$R/named" 2>/dev/null
waitfor '[ -s "$W/fdname" ]'
[ "$(cat "$W/fdname" 2>/dev/null)" = "1 varlink" ] && ok "Accept=yes: the connection is named after FileDescriptorName=" || bad "LISTEN_FDS/FDNAMES: $(cat "$W/fdname" 2>&1)"

expect_state pw.socket active/listening
expect_state plumb.service inactive/dead
python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.recv(1)' "$R/pw" 2>/dev/null
expect_state pw.service active/running
expect_state plumb.service active/running
case " $(SC show -p Wants --value pw.service) " in *" plumb.service "*) ok "pw.service's Wants= lists its .wants/ entry" ;; *) bad "pw.service Wants=$(SC show -p Wants --value pw.service)" ;; esac

echo "== slice D: MAINPID= hands the service over (podman)"
if SC start handover.service; then
    sleep 2
    want=$(cat "$W/handover.pid" 2>/dev/null)
    expect_state handover.service active/running
    got=$(SC show -p MainPID --value handover.service)
    [ -n "$want" ] && [ "$got" = "$want" ] && ok "MainPID is the announced process ($want)" || bad "MainPID $got, announced ${want:-none}"
    SC stop handover.service
    if [ -n "$want" ] && kill -0 "$want" 2>/dev/null; then bad "the announced process $want survived the stop"; else ok "the stop ended the announced process"; fi
else
    bad "handover.service: $(SC status handover.service 2>&1 | tail -3)"
fi

echo "== a transaction's unordered units start at once"
t0=$(date +%s%N)
SC start par.target || true
ms=$((($(date +%s%N) - t0) / 1000000))
[ "$ms" -lt 4500 ] && ok "three 2 s oneshots started in $ms ms (in parallel)" || bad "par.target took $ms ms (one at a time is 6000)"
if awk -v a="$(cat "$W/par1.end" 2>/dev/null)" -v b="$(cat "$W/parafter.start" 2>/dev/null)" 'BEGIN {exit !(a > 0 && b >= a)}'; then
    ok "the unit ordered After= one of them started after it ended"
else
    bad "order: par1 ended $(cat "$W/par1.end" 2>&1), parafter started $(cat "$W/parafter.start" 2>&1)"
fi

echo "== slice E: starts of one unit at once merge (systemd's job merging)"
calls=()
for _ in 1 2 3; do
    $B call $MGR StartUnit ss mergetgt.target replace-irreversibly >/dev/null 2>&1 &
    calls+=($!)
done
wait "${calls[@]}" || true
waitfor '[ -s "$W/counter" ]' || true
# A second run would end 3 s after the first.
sleep 4
[ "$(wc -l <"$W/counter")" = 1 ] && ok "three starts at once ran the wanted oneshot once" || bad "counter.service ran $(wc -l <"$W/counter") times"
call StartUnit ss mergetgt.target replace-irreversibly || true
waitfor '[ "$(wc -l <"$W/counter")" = 2 ]' && ok "a later start runs it again (the next logout)" || bad "counter.service ran $(wc -l <"$W/counter") times"

echo "== slice E: CollectMode=inactive-or-failed (dbus-broker's transient services)"
env XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" systemd-run --user --unit cf-fail.service \
    -p CollectMode=inactive-or-failed /bin/sh -c 'sleep 2; exit 1' >/dev/null 2>&1 || true
[ "$(SC show -p CollectMode --value cf-fail.service)" = inactive-or-failed ] && ok "CollectMode= is kept" || bad "CollectMode: $(SC show -p CollectMode --value cf-fail.service)"
env XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" systemd-run --user --unit cf-keep.service /bin/false >/dev/null 2>&1 || true
waitfor '[ "$(SC show -p ActiveState --value cf-keep.service)" = failed ]' || true
t=0
until [ "$(SC show -p LoadState --value cf-fail.service)" = not-found ] || [ $t -ge 30 ]; do sleep 1; t=$((t + 1)); done
[ "$(SC show -p LoadState --value cf-fail.service)" = not-found ] && [ $t -le 5 ] && ok "a failed inactive-or-failed unit is collected at once (${t} s)" || bad "cf-fail.service: $(SC show -p ActiveState,LoadState --value cf-fail.service | paste -sd/)"
[ "$(SC show -p ActiveState --value cf-keep.service)" = failed ] && ok "without it the failed unit stays" || bad "cf-keep.service: $(SC show -p ActiveState --value cf-keep.service)"
SC reset-failed cf-keep.service 2>/dev/null || true

echo "== systemd-run --scope (Ptyxis starts each shell this way)"
# systemd-run (258+) reads InvocationID at unit/self once it is in the scope;
# without that path it failed with "Unknown object" and Ptyxis fell back.
# A scope needs a delegated cgroup (CGROUP_BASE), which this sandbox only
# has when it runs inside one; the GNOME VM's apps mode checks it always.
if ! grep -q "^CGROUP_BASE=['\"]\?/" "$R/uinitconverter/uinitconverter.conf" 2>/dev/null; then
    echo "SKIP no delegated cgroup here (tests/vm-user-manager.sh apps checks it)"
elif env XDG_RUNTIME_DIR="$R" DBUS_SESSION_BUS_ADDRESS="unix:path=$R/bus" systemd-run --user --scope --quiet \
    --unit sr-scope.scope /bin/sh -c 'cat /proc/self/cgroup' >"$W/sr-scope.out" 2>&1; then
    ok "systemd-run --user --scope runs its command"
else
    bad "systemd-run --scope: $(cat "$W/sr-scope.out")"
fi

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
expect_state sessreq.service inactive/dead
expect_state sessmon.service inactive/dead
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

uid=$(id -u)
[ ! -e "$W/fb/$uid" ] && [ ! -e "$W/fb/$uid.running" ] && ok "a clean stop leaves no fallback record" || bad "fallback dir: $(ls -la "$W/fb")"

echo "== slice E: systemctl --user exit"
start_mgr "$W/daemon2.log"
waitfor 'grep -q "default.target done" "$W/daemon2.log"' || bad "second start: $(tail -3 "$W/daemon2.log")"
[ "$(cat "$W/fb/$uid.running" 2>/dev/null)" = "$MGR_PID" ] && ok "the running stamp holds the manager's PID" || bad "running stamp: $(cat "$W/fb/$uid.running" 2>&1)"
SC exit 2>/dev/null || true
waitfor '! kill -0 "$MGR_PID" 2>/dev/null' && ok "systemctl --user exit ended the manager" || bad "still running after exit: $(tail -3 "$W/daemon2.log")"
rc=0; wait "$MGR_PID" 2>/dev/null || rc=$?
[ "$rc" = 0 ] && [ ! -e "$W/fb/$uid" ] && [ ! -e "$W/fake-systemd.out" ] && ok "a clean exit (0), no fallback" || bad "exit: rc $rc, $(ls "$W/fb") $(cat "$W/fake-systemd.out" 2>&1)"

echo "== slice E: a unit fails into USER_FALLBACK_ON=: systemd --user takes over in place"
start_mgr "$W/daemon3.log"
waitfor 'grep -q "default.target done" "$W/daemon3.log"' || bad "third start: $(tail -3 "$W/daemon3.log")"
base_pid=$(SC show -p MainPID --value base.service)
SC start sessfail.service
waitfor '[ -s "$W/fake-systemd.out" ]' && ok "systemd --user was started" || bad "no fallback: $(tail -5 "$W/daemon3.log")"
[ "$(cut -d' ' -f1 "$W/fake-systemd.out" 2>/dev/null)" = "$MGR_PID" ] && ok "in the manager's own process (user@ goes on)" || bad "fake systemd: $(cat "$W/fake-systemd.out" 2>&1), manager $MGR_PID"
[ "$(cut -d' ' -f2 "$W/fake-systemd.out" 2>/dev/null)" = "--user" ] && ok "as systemd --user" || bad "arguments: $(cat "$W/fake-systemd.out" 2>&1)"
case $(cat "$W/fb/$uid" 2>/dev/null) in *"sessfail.service failed (OnFailure=fbdown.target)"*) ok "the reason is recorded: $(cat "$W/fb/$uid")" ;; *) bad "record: $(cat "$W/fb/$uid" 2>&1)" ;; esac
if kill -0 "$base_pid" 2>/dev/null; then bad "base.service PID $base_pid still runs"; else ok "the s6 units were stopped first"; fi
if s6-svscanctl -z "$R/uinitconverter/scan" 2>/dev/null; then bad "s6-svscan still runs"; else ok "s6-svscan ended"; fi
[ ! -e "$W/fb/$uid.running" ] && ok "no running stamp is left" || bad "running stamp left"
kill "$MGR_PID" 2>/dev/null; wait "$MGR_PID" 2>/dev/null || true

echo "== slice E: a later start in this boot goes straight to systemd --user"
rm -f "$W/fake-systemd.out"
start_mgr "$W/daemon4.log"
waitfor '[ -s "$W/fake-systemd.out" ]' && [ "$(cut -d' ' -f1 "$W/fake-systemd.out")" = "$MGR_PID" ] && ok "handed over at once" || bad "no hand-over: $(tail -3 "$W/daemon4.log")"
grep -q "fell back earlier in this boot" "$W/daemon4.log" && ok "and says why" || bad "daemon4.log: $(tail -3 "$W/daemon4.log")"
kill "$MGR_PID" 2>/dev/null; wait "$MGR_PID" 2>/dev/null || true

echo "== slice E: a crash (a stale running stamp) and a failed start of a fallback target"
rm -f "$W/fake-systemd.out" "$W/fb/$uid"
echo 999999 >"$W/fb/$uid.running"
start_mgr "$W/daemon5.log"
waitfor '[ -s "$W/fake-systemd.out" ]' && ok "a stale running stamp: systemd --user" || bad "no hand-over: $(tail -3 "$W/daemon5.log")"
case $(cat "$W/fb/$uid" 2>/dev/null) in *"PID 999999"*"ended without stopping"*) ok "recorded as a crash" ;; *) bad "record: $(cat "$W/fb/$uid" 2>&1)" ;; esac
kill "$MGR_PID" 2>/dev/null; wait "$MGR_PID" 2>/dev/null || true
rm -f "$W/fake-systemd.out" "$W/fb/$uid" "$W/fb/$uid.running"
start_mgr "$W/daemon6.log"
waitfor 'grep -q "default.target done" "$W/daemon6.log"' && [ ! -e "$W/fake-systemd.out" ] && ok "without a record the s6 manager runs again" || bad "daemon6.log: $(tail -3 "$W/daemon6.log")"
# A logout under way (the OnFailure= target is up already): no fallback.
SC start fbdown.target
SC start sessfail.service 2>/dev/null || true
waitfor 'grep -q "is under way: no fallback" "$W/daemon6.log"' && [ ! -e "$W/fake-systemd.out" ] \
    && ok "a failure into a target already up (a logout) is no fallback" || bad "daemon6.log: $(tail -3 "$W/daemon6.log"), fallback: $(cat "$W/fake-systemd.out" 2>&1)"
SC stop fbdown.target
SC reset-failed sessfail.service 2>/dev/null || true
SC start fbsess@x.target 2>/dev/null || true
waitfor '[ -s "$W/fake-systemd.out" ]' && ok "a failed start of fbsess@x.target: systemd --user" || bad "no hand-over: $(tail -5 "$W/daemon6.log")"
case $(cat "$W/fb/$uid" 2>/dev/null) in *"fbsess@x.target ended"*) ok "recorded: $(cat "$W/fb/$uid")" ;; *) bad "record: $(cat "$W/fb/$uid" 2>&1)" ;; esac
kill "$MGR_PID" 2>/dev/null; wait "$MGR_PID" 2>/dev/null || true

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
