#!/bin/sh
# Unprivileged end-to-end test: private s6-svscan + s6-rc live state + private
# D-Bus bus. Does not touch the host's init system.
#
#   tests/sandbox.sh [WORKDIR]      set up and start everything
#   tests/sandbox.sh WORKDIR stop   tear it down
# SANDBOX_JOURNAL=no sets it up as a JOURNAL=no install (per-service s6-log).
set -eu

BIN=$(cd "$(dirname "$0")/.." && pwd)/target/debug/sdcompat
W=${1:-${TMPDIR:-/tmp}/sdcompat-sandbox}
# Sockets need short paths (sun_path is 108 bytes).
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-test

if [ "${2:-}" = stop ]; then
    # The daemon's PID is kept apart: tests/check.sh replaces the daemon.
    # shellcheck disable=SC2046  # PID lists, split on purpose
    [ -f "$W/pids" ] && kill $(cat "$W/pids") $(cat "$W/daemon.pid" 2>/dev/null) 2>/dev/null || true
    sleep 0.5
    rm -rf "$W" "$R"
    exit 0
fi

rm -rf "$W" "$R"
mkdir -p "$W/units" "$W/presets" "$W/scan" "$R"
U=$W/units

cat > "$W/sdcompat.conf" <<EOF
UNIT_PATH=$U
RUNTIME_UNIT_DIR=$R/units
PRESET_PATH=$W/presets
EXTRA_SOURCE=
STATE_DIR=$W/state
LIVE_DIR=$R/live
RUN_DIR=$R/run
LOG_DIR=$W/logs
BOOTED_FLAG=
PRIVATE_SOCKET=$R/private
BUS_ADDRESS=unix:path=$R/bus
ALLOW_UIDS=$(id -u)
RUNNER=$BIN
POWEROFF_CMD=touch $R/poweroff-requested
REBOOT_CMD=touch $R/reboot-requested
JOURNAL=${SANDBOX_JOURNAL:-yes}
JOURNAL_STDOUT=$R/jstdout
TRANSIENT_DIR=$R/transient
UDEV_DIR=$W/udev
UDEV_MONITOR=no
EOF

cat > "$U/multi-user.target" <<EOF
[Unit]
Description=Multi-User System
AllowIsolate=yes
EOF
ln -s multi-user.target "$U/default.target"

cat > "$U/hello.service" <<EOF
[Unit]
Description=Simple long-running service
[Service]
ExecStart=/bin/sh -c 'echo hello from \$\$; exec sleep 1000'
Environment="GREETING=hi there"
[Install]
WantedBy=multi-user.target
Alias=hello-alias.service
EOF

cat > "$U/notifier.service" <<EOF
[Unit]
Description=Type=notify service
[Service]
Type=notify
ExecStart=/bin/sh -c 'sleep 0.3; $BIN notify --ready --status="all good"; exec sleep 1000'
[Install]
WantedBy=multi-user.target
EOF

cat > "$U/once.service" <<EOF
[Unit]
Description=Oneshot with RemainAfterExit
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'echo once > $R/once.out'
ExecStop=/bin/sh -c 'echo stopped >> $R/once.out'
EOF

# SystemCallFilter=: a deny list, fwupd's allow list (fwupd checks that
# mlock fails) and a '+' command, which is not filtered.
cat > "$U/scf-deny.service" <<EOF
[Service]
Type=oneshot
SystemCallFilter=~kill
SystemCallErrorNumber=EPERM
ExecStart=/bin/sh -c 'if kill -0 \$\$\$\$ 2>/dev/null; then echo allowed; else echo blocked; fi > $R/scf-deny.out'
ExecStartPost=+/bin/sh -c 'if kill -0 \$\$\$\$ 2>/dev/null; then echo allowed; else echo blocked; fi > $R/scf-plus.out'
EOF

cat > "$U/scf-allow.service" <<EOF
[Service]
Type=oneshot
SystemCallFilter=@basic-io @file-system @io-event @ipc @network-io @process @sync @signal @timer @chown ioctl uname fadvise64 sysinfo madvise mremap splice vmsplice copy_file_range
SystemCallErrorNumber=EPERM
SystemCallArchitectures=native
ExecStart=/usr/bin/python3 -c 'import ctypes; c = ctypes.CDLL(None, use_errno=True); b = ctypes.create_string_buffer(4096); r = c.mlock(b, 4096); open("$R/scf-allow.out", "w").write("ok" if r == 0 else "errno %%d" % ctypes.get_errno())'
EOF

# The other sandboxing settings. The seccomp-based ones work unprivileged;
# the namespaces and mounts only as root (check.sh skips those otherwise).
cat > "$W/harden.py" <<'PYT'
import ctypes, errno, os, socket, sys
out = []
def t(name, f):
    try:
        f()
        out.append(f"{name} allowed")
    except OSError as e:
        out.append(f"{name} {errno.errorcode.get(e.errno, e.errno)}")
t("inet", lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM))
t("unix", lambda: socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM))
libc = ctypes.CDLL(None, use_errno=True)
libc.mmap.restype = ctypes.c_void_p
p = libc.mmap(None, 4096, 7, 0x22, -1, 0)
out.append("mmap-wx " + ("allowed" if p not in (None, 2**64 - 1) else errno.errorcode[ctypes.get_errno()]))
f = os.path.join(sys.argv[2], "harden-suid")
open(f, "w").close()
t("chmod-suid", lambda: os.chmod(f, 0o4755))
t("unshare-user", lambda: os.unshare(os.CLONE_NEWUSER))
t("procsys", lambda: open("/proc/sys/kernel/domainname", "w").write("x"))
out.append("dev " + " ".join(sorted(os.listdir("/dev"))))
out.append("netdevs " + " ".join(l.split(":")[0].strip() for l in open("/proc/net/dev").readlines()[2:]))
open(sys.argv[1], "w").write("\n".join(out) + "\n")
PYT
cat > "$U/harden.service" <<EOF
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 $W/harden.py $R/harden.out $R
RestrictAddressFamilies=AF_UNIX
MemoryDenyWriteExecute=yes
RestrictSUIDSGID=yes
RestrictNamespaces=yes
LockPersonality=yes
RestrictRealtime=yes
PrivateDevices=yes
PrivateNetwork=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectHostname=yes
ProtectClock=yes
EOF

cat > "$U/dep.service" <<EOF
[Unit]
Description=Depends on once.service
Requires=once.service
After=once.service
[Service]
ExecStart=/bin/sleep 1000
EOF

cat > "$U/failing.service" <<EOF
[Unit]
Description=Exits with status 3
[Service]
ExecStart=/bin/sh -c 'exit 3'
EOF

# Fails on its first run, exits 0 on the second (Result= must not stay
# exit-code from the first run).
cat > "$U/second.service" <<EOF
[Unit]
Description=Fails once, then exits 0
[Service]
ExecStart=/bin/sh -c '[ -e $R/second.ran ] && exit 0; touch $R/second.ran; exit 4'
EOF

cat > "$U/flappy.service" <<EOF
[Unit]
Description=Restart=on-failure with a start limit
StartLimitBurst=2
StartLimitIntervalSec=60
[Service]
ExecStart=/bin/sh -c 'exit 4'
Restart=on-failure
RestartSec=0
EOF

cat > "$U/cond.service" <<EOF
[Unit]
Description=Condition fails
ConditionPathExists=/nonexistent/sdcompat
[Service]
ExecStart=/bin/sleep 1000
EOF

cat > "$U/sock.service" <<EOF
[Unit]
Description=Socket-activated service
[Service]
ExecStart=/bin/sh -c 'echo "fds=\$LISTEN_FDS names=\$LISTEN_FDNAMES pid=\$LISTEN_PID self=\$\$" > $R/sock.out; exec sleep 1000'
EOF
cat > "$U/sock.socket" <<EOF
[Socket]
ListenStream=$R/test.sock
[Install]
WantedBy=sockets.target
EOF

cat > "$U/forking.service" <<EOF
[Unit]
Description=Type=forking with PIDFile
[Service]
Type=forking
PIDFile=$R/fork.pid
ExecStart=/bin/sh -c 'sleep 1000 & echo \$! > $R/fork.pid'
EOF

cat > "$U/reloadable.service" <<EOF
[Unit]
Description=Has ExecReload
[Service]
ExecStart=/bin/sh -c 'trap "echo reloaded >> $R/reload.out" HUP; while :; do sleep 1; done'
ExecReload=/bin/kill -HUP \$MAINPID
EOF

# Like user@.service: the reload is done when READY=1 follows RELOADING=1.
cat > "$U/notreload.service" <<EOF
[Unit]
Description=Type=notify-reload
[Service]
Type=notify-reload
ReloadSignal=USR1
ExecStart=/bin/sh -c 'trap "$BIN notify --reloading; sleep 1; echo reloaded >> $R/notreload.out; $BIN notify --ready" USR1; $BIN notify --ready; while :; do sleep 0.1; done'
EOF

# --- timers ---
for n in tick cal persist later; do
cat > "$U/$n.service" <<EOF
[Unit]
Description=Work for $n.timer
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'date +%%s >> $R/$n.out'
EOF
done
cat > "$U/tick.timer" <<EOF
[Unit]
Description=Monotonic timer
[Timer]
OnBootSec=1
OnUnitActiveSec=2s
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/cal.timer" <<EOF
[Unit]
Description=Calendar timer
[Timer]
OnCalendar=*:*:0/3
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/persist.timer" <<EOF
[Unit]
Description=Persistent timer that missed a run
[Timer]
OnCalendar=yearly
Persistent=true
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/later.timer" <<EOF
[Unit]
Description=Far-future timer
[Timer]
OnCalendar=2099-01-01 00:00:00
EOF
# --- on-demand template instances (like user@UID.service) ---
cat > "$U/prep@.service" <<EOF
[Unit]
Description=Prepare %i (like user-runtime-dir@)
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'echo up %i >> $R/prep.out; [ %i != d ] || sleep 1'
ExecStop=/bin/sh -c 'echo down %i >> $R/prep.out'
EOF
cat > "$U/inst@.service" <<EOF
[Unit]
Description=Instance %i (like user@)
BindsTo=prep@%i.service
After=prep@%i.service
[Service]
Type=notify
ExecStart=/bin/sh -c 'echo instance %i up; $BIN notify --ready; exec sleep 1000'
EOF

# --- Conflicts= (like gdm.service vs getty@tty1.service) ---
cat > "$U/winner.service" <<EOF
[Unit]
Description=Declares the conflict
Conflicts=loser.service
[Service]
ExecStart=/bin/sleep 1000
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/loser.service" <<EOF
[Unit]
Description=Conflicted unit
[Service]
ExecStart=/bin/sleep 1000
[Install]
WantedBy=multi-user.target
EOF

# --- socket matched by Service= (like systemd-logind-varlink.socket) ---
cat > "$U/vl.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'echo "fds=\$LISTEN_FDS names=\$LISTEN_FDNAMES" > $R/vl.out; exec sleep 1000'
EOF
cat > "$U/vl-varlink.socket" <<EOF
[Socket]
ListenStream=$R/vl.sock
FileDescriptorName=varlink
Service=vl.service
[Install]
WantedBy=sockets.target
EOF

# --- OnFailure= / FailureAction= (offline-update style) ---
cat > "$U/upd.service" <<EOF
[Unit]
Description=Fails, starts its cleanup and asks for a reboot
OnFailure=upd-cleanup.service
FailureAction=reboot
[Service]
Type=oneshot
ExecStart=/bin/false
EOF
cat > "$U/upd-cleanup.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo cleaned > $R/cleanup.out'
EOF

# --- failure propagation (s6-rc only orders; sdcompat enforces Requires=) ---
cat > "$U/bad.service" <<EOF
[Unit]
Description=Fails unless \$R/bad-ok exists
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'test -e $R/bad-ok'
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/after-bad.service" <<EOF
[Unit]
Description=Ordered after a failing wanted unit
After=bad.service
Wants=bad.service
[Service]
ExecStart=/bin/sleep 1000
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/needs-bad.service" <<EOF
[Unit]
Description=Requires the failing unit
Requires=bad.service
After=bad.service
[Service]
ExecStart=/bin/sleep 1000
EOF
cat > "$U/t.target" <<EOF
[Unit]
Wants=bad.service
EOF
cat > "$U/consumer.service" <<EOF
[Unit]
Requires=t.target
After=t.target
[Service]
ExecStart=/bin/sleep 1000
EOF
cat > "$U/cond2.service" <<EOF
[Unit]
ConditionPathExists=$R/cond-flag
[Service]
ExecStart=/bin/sleep 1000
EOF
cat > "$U/slow.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sleep 3
EOF

# --- fd store: FDSTORE=1 descriptors survive a restart ---
cat > "$W/fdstore-test.py" <<'PYT'
import os, socket, sys
out = sys.argv[1]
names = os.environ.get("LISTEN_FDNAMES", "")
with open(out, "a") as f:
    f.write("start names=%s\n" % names)
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
addr = os.environ["NOTIFY_SOCKET"]
if addr.startswith("@"):
    addr = "\0" + addr[1:]
if not names:
    fd = os.open(out, os.O_RDONLY)
    s.sendmsg([b"FDSTORE=1\nFDNAME=kept"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fd.to_bytes(4, sys.byteorder))], 0, addr)
s.sendto(b"READY=1", addr)
os.execv("/bin/sleep", ["sleep", "1000"])
PYT
cat > "$U/fdkeep.service" <<EOF
[Service]
Type=notify
FileDescriptorStoreMax=4
ExecStart=/usr/bin/python3 $W/fdstore-test.py $R/fdkeep.out
EOF

# --- D-Bus activation through dbus-daemon --systemd-activation ---
mkdir -p "$W/dbus-services"
cat > "$W/dbus-services/org.example.Act.service" <<EOF
[D-BUS Service]
Name=org.example.Act
Exec=/bin/false
SystemdService=act.service
EOF
cat > "$U/act.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'echo activated >> $R/act.out; exec sleep 1000'
EOF

# --- units wanted by devices (udev SYSTEMD_WANTS), from a fake udev database ---
mkdir -p "$W/udev/tags/systemd" "$W/udev/data"
udev_dev() { # udev_dev ID DATA-LINES...
    id=$1
    shift
    touch "$W/udev/tags/systemd/$id"
    printf '%s\n' "$@" > "$W/udev/data/$id"
}
udev_dev +test:dev0 "E:SYSTEMD_WANTS=devwant" "G:systemd" "Q:systemd"
udev_dev +test:dev1 "E:SYSTEMD_WANTS=devnotready.service" "E:SYSTEMD_READY=0" "G:systemd"
udev_dev +test:dev2 "E:SYSTEMD_WANTS=devuntagged.service" "G:systemd" "Q:seat"
for n in devwant devnotready devuntagged devlater; do
cat > "$U/$n.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
done

# --- starting beside a busy s6-rc change ---
# hold.service is ready only once $R/hold-<name> exists, so an s6-rc change
# that starts it keeps the s6-rc lock until then.
cat > "$W/wait-ready.py" <<'PYT'
import os, socket, sys, time
while not os.path.exists(sys.argv[1]):
    time.sleep(0.1)
addr = os.environ["NOTIFY_SOCKET"]
if addr.startswith("@"):
    addr = "\0" + addr[1:]
socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).sendto(b"READY=1", addr)
os.execv("/bin/sleep", ["sleep", "1000"])
PYT
for n in 1 2 3 4; do
cat > "$U/hold$n.service" <<EOF
[Service]
Type=notify
ExecStart=/usr/bin/python3 $W/wait-ready.py $R/hold$n-ready
EOF
done
# Makes hold1 ready, like polkit made upowerd ready at boot.
cat > "$U/beside1.service" <<EOF
[Service]
ExecStart=/bin/sh -c 'echo beside1 up; touch $R/hold1-ready; exec sleep 1000'
EOF
cat > "$U/beside2.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
# A target started while hold3's change keeps the lock (udev wanting
# bluetooth.target while the boot waits for the network).
cat > "$U/beside3.service" <<EOF
[Service]
ExecStart=/bin/sleep 1000
EOF
cat > "$U/beside.target" <<EOF
[Unit]
Wants=beside3.service
EOF

# --- socket started on its own, re-activated after its service exits ---
# Like systemd-rfkill.socket: a static socket with ListenSpecial= (here a
# FIFO), whose service handles what is queued and exits when idle.
mkfifo "$R/react.fifo"
cat > "$W/react.py" <<'PYT'
import os, sys
assert os.environ.get("LISTEN_FDS") == "1", os.environ.get("LISTEN_FDS")
try:
    data = os.read(3, 100).decode().strip()
except BlockingIOError:
    data = "-"
with open(sys.argv[1], "a") as f:
    f.write(data + "\n")
PYT
cat > "$U/react.socket" <<EOF
[Socket]
ListenSpecial=$R/react.fifo
Writable=yes
EOF
cat > "$U/react.service" <<EOF
[Service]
ExecStart=/usr/bin/python3 $W/react.py $R/react.out
EOF
# The same, started beside hold4's change (systemd-rfkill at boot).
mkfifo "$R/react2.fifo"
cat > "$U/react2.socket" <<EOF
[Socket]
ListenSpecial=$R/react2.fifo
Writable=yes
EOF
cat > "$U/react2.service" <<EOF
[Service]
ExecStart=/usr/bin/python3 $W/react.py $R/react2.out
EOF

# --- path units ---
cat > "$U/watch.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo started >> $R/watch.out; rm -f $R/flag'
EOF
cat > "$U/watch.path" <<EOF
[Unit]
Description=Level-triggered path unit
[Path]
PathExists=$R/flag
[Install]
WantedBy=multi-user.target
EOF
cat > "$U/chg.service" <<EOF
[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo changed >> $R/chg.out'
EOF
cat > "$U/chg.path" <<EOF
[Unit]
Description=Edge-triggered path unit
[Path]
PathChanged=$R/chgdir
MakeDirectory=yes
[Install]
WantedBy=multi-user.target
EOF

mkdir -p "$W/state/timers"
echo 1000000000000000 > "$W/state/timers/persist.timer"

cat > "$W/presets/90-test.preset" <<EOF
enable notifier.service
disable *
EOF

C="--config $W/sdcompat.conf"
"$BIN" $C enable hello.service sock.socket tick.timer cal.timer persist.timer winner.service loser.service vl-varlink.socket watch.path chg.path bad.service after-bad.service >/dev/null

# Stand-in for journald's stdout stream socket: records each connection.
mkdir -p "$R/journal"
python3 -c '
import os, socket, sys, threading
path, out = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path); s.listen(64)
n = 0
def handle(c, i):
    with open(os.path.join(out, "%d.log" % i), "ab", buffering=0) as f:
        while True:
            d = c.recv(65536)
            if not d:
                break
            f.write(d)
while True:
    c, _ = s.accept(); n += 1
    threading.Thread(target=handle, args=(c, n), daemon=True).start()
' "$R/jstdout" "$R/journal" &
echo $! > "$W/pids"
sleep 0.3
"$BIN" $C preset notifier.service >/dev/null
"$BIN" $C reload --no-update

# The fd store runs apart from the daemon, like the sdcompat-fdstore service.
"$BIN" $C fdstore >"$W/fdstore.log" 2>&1 &
echo $! >> "$W/pids"
sleep 0.3

# JOURNAL=no: the journal-relay service, forwarding to a stand-in /dev/log
# that writes what it gets to $R/devlog.out.
if [ "${SANDBOX_JOURNAL:-yes}" = no ]; then
    python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.bind(sys.argv[1])
with open(sys.argv[2], "ab", buffering=0) as f:
    while True:
        f.write(s.recv(65536) + b"\n")
' "$R/devlog" "$R/devlog.out" >/dev/null 2>&1 &
    echo $! >> "$W/pids"
    "$BIN" journal-relay --dir "$R/jrelay" --syslog "$R/devlog" >"$W/jrelay.log" 2>&1 &
    echo $! >> "$W/pids"
    sleep 0.3
fi

s6-svscan "$W/scan" >"$W/svscan.log" 2>&1 &
echo $! >> "$W/pids"
sleep 0.3
s6-rc-init -c "$W/state/compiled" -l "$R/live" "$W/scan"
s6-rc -l "$R/live" -u change sdcompat-default || true

cat > "$W/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$R/bus</listen>
  <servicedir>$W/dbus-services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
dbus-daemon --config-file="$W/bus.conf" --systemd-activation --nofork --nopidfile >"$W/dbus.log" 2>&1 &
echo $! >> "$W/pids"
sleep 0.3
"$BIN" $C daemon --address "unix:path=$R/bus" >"$W/daemon.log" 2>&1 &
echo $! > "$W/daemon.pid"
sleep 0.5

echo "sandbox ready"
echo "  bus:     unix:path=$R/bus"
echo "  config:  $W/sdcompat.conf"
echo "  live:    $R/live"
