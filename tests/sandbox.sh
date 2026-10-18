#!/bin/sh
# Unprivileged end-to-end test: private s6-svscan + s6-rc live state + private
# D-Bus bus. Does not touch the host's init system.
#
#   tests/sandbox.sh [WORKDIR]      set up and start everything
#   tests/sandbox.sh WORKDIR stop   tear it down
set -eu

BIN=$(cd "$(dirname "$0")/.." && pwd)/target/debug/sdcompat
W=${1:-${TMPDIR:-/tmp}/sdcompat-sandbox}
# Sockets need short paths (sun_path is 108 bytes).
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-test

if [ "${2:-}" = stop ]; then
    [ -f "$W/pids" ] && kill $(cat "$W/pids") 2>/dev/null || true
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
ExecStart=/bin/sh -c 'echo up %i >> $R/prep.out'
ExecStop=/bin/sh -c 'echo down %i >> $R/prep.out'
EOF
cat > "$U/inst@.service" <<EOF
[Unit]
Description=Instance %i (like user@)
BindsTo=prep@%i.service
After=prep@%i.service
[Service]
Type=notify
ExecStart=/bin/sh -c '$BIN notify --ready; exec sleep 1000'
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

mkdir -p "$W/state/timers"
echo 1000000000000000 > "$W/state/timers/persist.timer"

cat > "$W/presets/90-test.preset" <<EOF
enable notifier.service
disable *
EOF

C="--config $W/sdcompat.conf"
"$BIN" $C enable hello.service sock.socket tick.timer cal.timer persist.timer winner.service loser.service vl-varlink.socket >/dev/null
"$BIN" $C preset notifier.service >/dev/null
"$BIN" $C reload --no-update

s6-svscan "$W/scan" >"$W/svscan.log" 2>&1 &
echo $! > "$W/pids"
sleep 0.3
s6-rc-init -c "$W/state/compiled" -l "$R/live" "$W/scan"
s6-rc -l "$R/live" -u change sdcompat-default || true

dbus-daemon --session --address="unix:path=$R/bus" --nofork --nopidfile >"$W/dbus.log" 2>&1 &
echo $! >> "$W/pids"
sleep 0.3
"$BIN" $C daemon --address "unix:path=$R/bus" >"$W/daemon.log" 2>&1 &
echo $! >> "$W/pids"
sleep 0.5

echo "sandbox ready"
echo "  bus:     unix:path=$R/bus"
echo "  config:  $W/sdcompat.conf"
echo "  live:    $R/live"
