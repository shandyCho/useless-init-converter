#!/bin/sh
# Scenario checks against a running sandbox (see tests/sandbox.sh).
#   tests/check.sh WORKDIR
set -u

W=$1
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-test
# The unlabelled s6-rc-update copies (see sandbox.sh), for the daemon this
# script starts and its own s6-rc-set-* calls.
PATH=$W/bin:$PATH
export PATH
B="busctl --address=unix:path=$R/bus"
DEST=org.freedesktop.systemd1
MGR="$DEST /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager"
pass=0
fail=0

# The checks expect the state right after tests/sandbox.sh (bad.service
# failed, output files empty, ...), so each sandbox can be checked once.
if [ -e "$W/checked" ]; then
    echo "check.sh: $W was already checked; recreate the sandbox first:" >&2
    echo "  tests/sandbox.sh $W stop && tests/sandbox.sh $W" >&2
    exit 2
fi
touch "$W/checked"

ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }

upath() { $B call $MGR GetUnit s "$1" | cut -d'"' -f2; }
prop() { $B get-property $DEST "$(upath "$1")" "org.freedesktop.systemd1.$2" "$3" | cut -d' ' -f2- | tr -d '"'; }
state() { echo "$(prop "$1" Unit ActiveState)/$(prop "$1" Unit SubState)"; }
call() { $B call $MGR "$@" >/dev/null 2>"$W/last-error" || { cat "$W/last-error"; return 1; }; }

expect_state() {
    unit=$1 want=$2 tries=${3:-30}
    while [ "$tries" -gt 0 ]; do
        got=$(state "$unit")
        [ "$got" = "$want" ] && { ok "$unit is $want"; return; }
        tries=$((tries - 1))
        sleep 0.2
    done
    bad "$unit: expected $want, got $got"
}

expect_eq() {
    [ "$2" = "$3" ] && ok "$1" || bad "$1: expected '$3', got '$2'"
}

lines() { wc -l 2>/dev/null <"$1" || echo 0; }
# last_new FILE COUNT: FILE's last line if it grew past COUNT lines, so a
# line left by an earlier run doesn't pass for the new one.
last_new() {
    if [ "$(lines "$1")" -gt "$2" ]; then tail -1 "$1"; else echo "(no new line)"; fi
}

# SANDBOX_JOURNAL=no: output goes to per-service s6-log directories.
grep -qx 'JOURNAL=no' "$W/sdcompat.conf" && journal=no || journal=yes
expect_log() {
    unit=$1 text=$2 tries=25
    while [ $tries -gt 0 ] && ! grep -q "$text" "$W/logs/$unit/current" 2>/dev/null; do
        tries=$((tries - 1))
        sleep 0.2
    done
    grep -q "$text" "$W/logs/$unit/current" 2>/dev/null && ok "$unit output in its s6-log" || bad "no '$text' in $W/logs/$unit/current"
}

# Damage one failure leaves behind for everything after it. On the real
# boot a failed s6-rc-update moved a service directory out of the live
# directory: its scan-directory link dangled, s6-svscan aborted every
# scan, stops waited for it forever, and so did sudo. Prints one line per
# problem found.
damage() {
    live=$(readlink "$R/live")
    for d in "$R"/live:*; do
        [ -e "$d" ] && [ "${d##*/}" != "$live" ] && echo "s6-rc live directory left over: ${d##*/}"
    done
    [ "$(ls "$R/live/compiled/servicedirs")" = "$(ls "$R/live/servicedirs")" ] \
        || echo "live service directories differ from the compiled database"
    for l in "$(readlink -f "$R/live/scandir")"/*; do
        [ -L "$l" ] && [ ! -e "$l" ] && echo "dangling scan directory entry: ${l##*/}"
    done
    # s6-rc's own services never stop by themselves (units may: s6-rc
    # doesn't follow deaths, and a failed unit stays "up" for it). After
    # a failed then retried s6-rc-update, s6rc-fdholder stayed down and
    # every logger start failed. s6-rc doesn't wait for its lock here:
    # while a section holds it, skip.
    up=$(s6-rc -l "$R/live" -a list 2>/dev/null) || return 0
    for s in $up; do
        case $s in s6rc-*) ;; *) continue ;; esac
        [ "$(s6-svstat -o up "$R/live/servicedirs/$s" 2>/dev/null)" = true ] \
            || echo "s6-rc has $s up, but its supervisor doesn't run it"
    done
}
# Each kind of damage fails once, at the first section boundary it shows at.
: > "$W/damage.seen"
report_damage() {
    damage > "$W/damage.now"
    while IFS= read -r line; do
        grep -qxF "$line" "$W/damage.seen" && continue
        echo "$line" >> "$W/damage.seen"
        bad "damage before \"$1\": $line"
    done < "$W/damage.now"
}
section() {
    report_damage "$1"
    echo "== $1"
}

section "boot state"
expect_state hello.service active/running
expect_state notifier.service active/running
expect_eq "notify STATUS= relayed" "$(prop notifier.service Service StatusText)" "all good"
# The notify helper must block in poll() after READY=1, not spin.
ticks() {
    t=0
    for p in $(pgrep -u "$(id -u)" -f 'sdcompat-runner run .*-- notifier\.service$'); do
        t=$((t + $(awk '{print $14 + $15}' "/proc/$p/stat" 2>/dev/null || echo 0)))
    done
    echo "$t"
}
t0=$(ticks)
sleep 1
t1=$(ticks)
[ $((t1 - t0)) -lt 10 ] && ok "notify helper idle" || bad "notify helper used $((t1 - t0)) ticks in 1 s"
expect_eq "alias resolves" "$(upath hello-alias.service)" "/org/freedesktop/systemd1/unit/hello_2eservice"
expect_eq "UnitFileState enabled" "$(prop hello.service Unit UnitFileState)" "enabled"
expect_eq "preset disabled" "$(prop failing.service Unit UnitFilePreset)" "disabled"

section "start jobs (in parallel)"
for u in dep.service failing.service flappy.service cond.service sock.service forking.service reloadable.service; do
    call StartUnit ss "$u" replace &
done
wait
expect_state dep.service active/running
expect_state once.service active/exited
expect_eq "oneshot ran" "$(cat "$R/once.out" 2>/dev/null)" "once"
expect_state failing.service failed/failed
expect_eq "failing Result" "$(prop failing.service Service Result)" "exit-code"
expect_eq "failing ExecMainStatus" "$(prop failing.service Service ExecMainStatus)" "3"
expect_state flappy.service failed/failed 60
call StartUnit ss second.service replace >/dev/null
expect_state second.service failed/failed
call StartUnit ss second.service replace >/dev/null
expect_state second.service inactive/dead
expect_eq "second run's Result" "$(prop second.service Service Result)" "success"
expect_eq "flappy hit start limit" "$(prop flappy.service Service Result)" "start-limit-hit"
expect_state cond.service inactive/dead
expect_eq "condition result" "$(prop cond.service Unit ConditionResult)" "false"
expect_state sock.service active/running
case "$(cat "$R/sock.out" 2>/dev/null)" in
    "fds=1 names=sock.socket"*) ok "LISTEN_FDS passed" ;;
    *) bad "LISTEN_FDS: $(cat "$R/sock.out" 2>/dev/null)" ;;
esac
expect_state forking.service active/running
expect_eq "forking MainPID from PIDFile" "$(prop forking.service Service MainPID)" "$(cat "$R/fork.pid")"

section "MemoryPressureWatch="
call StartUnit ss psi-off.service replace
call StartUnit ss psi-auto.service replace
expect_state psi-off.service inactive/dead
expect_eq "off: MEMORY_PRESSURE_WATCH=/dev/null" "$(cat "$R/psi-off.out" 2>/dev/null)" "/dev/null"
expect_state psi-auto.service inactive/dead
expect_eq "auto without a cgroup: unset" "$(cat "$R/psi-auto.out" 2>/dev/null)" "unset"

section "SystemCallFilter="
call StartUnit ss scf-deny.service replace
call StartUnit ss scf-allow.service replace
expect_state scf-deny.service inactive/dead
expect_eq "deny list blocks kill with EPERM" "$(cat "$R/scf-deny.out" 2>/dev/null)" "blocked"
expect_eq "'+' command is not filtered" "$(cat "$R/scf-plus.out" 2>/dev/null)" "allowed"
expect_state scf-allow.service inactive/dead
expect_eq "fwupd's allow list: mlock fails with EPERM" "$(cat "$R/scf-allow.out" 2>/dev/null)" "errno 1"

section "other sandboxing settings"
call StartUnit ss harden.service replace
expect_state harden.service inactive/dead
h() { sed -n "s/^$1 //p" "$R/harden.out" 2>/dev/null; }
expect_eq "RestrictAddressFamilies=AF_UNIX: AF_INET refused" "$(h inet)" "EAFNOSUPPORT"
expect_eq "RestrictAddressFamilies=AF_UNIX: AF_UNIX allowed" "$(h unix)" "allowed"
# PR_SET_MDWE gives EACCES, the seccomp fallback EPERM.
case $(h mmap-wx) in EACCES|EPERM) ok "MemoryDenyWriteExecute=: W+X mmap refused" ;; *) bad "MemoryDenyWriteExecute=: mmap W+X $(h mmap-wx)" ;; esac
expect_eq "RestrictSUIDSGID=: chmod u+s refused" "$(h chmod-suid)" "EPERM"
expect_eq "RestrictNamespaces=yes: unshare refused" "$(h unshare-user)" "EPERM"
if [ "$(id -u)" = 0 ]; then
    expect_eq "ProtectKernelTunables=: /proc/sys read-only" "$(h procsys)" "EROFS"
    expect_eq "PrivateDevices=: only pseudo devices" "$(h dev)" "fd full hugepages log mqueue null ptmx pts random shm stderr stdin stdout tty urandom zero"
    expect_eq "PrivateNetwork=: only lo" "$(h netdevs)" "lo"
else
    echo "SKIP namespace and mount settings (need root)"
fi

section "system state"
expect_eq "SystemState degraded" "$($B get-property $MGR SystemState | cut -d'"' -f2)" "degraded"
# failing, flappy, and bad.service (fails at boot on purpose)
expect_eq "NFailedUnits" "$($B get-property $MGR NFailedUnits | cut -d' ' -f2)" "3"

section "reload / restart / stop"
call ReloadUnit ss reloadable.service replace && sleep 1.5
expect_eq "ExecReload ran" "$(cat "$R/reload.out" 2>/dev/null)" "reloaded"
call ReloadUnit ss hello.service replace 2>/dev/null && bad "reload without ExecReload accepted" || ok "reload without ExecReload rejected"
old=$(prop hello.service Service MainPID)
call RestartUnit ss hello.service replace
sleep 0.3
expect_state hello.service active/running
new=$(prop hello.service Service MainPID)
[ "$old" != "$new" ] && [ "$new" != 0 ] && ok "restart changed MainPID ($old -> $new)" || bad "restart: $old -> $new"
call StopUnit ss once.service replace
expect_state once.service inactive/dead
expect_eq "ExecStop ran" "$(tail -1 "$R/once.out")" "stopped"
expect_state dep.service inactive/dead
call ResetFailedUnit s failing.service
expect_state failing.service inactive/dead

section "ExecStop= when s6-rc stops a longrun"
L="s6-rc -l $R/live"
in_s6rc() { $L -a list | grep -qx "$1"; }
in_s6rc stopper.service.stop && ok "the .stop oneshot is up after boot" || bad "stopper.service.stop not up after boot"
pid=$(prop stopper.service Service MainPID)
$L -d change stopper.service
expect_eq "s6-rc -d ran ExecStop= with \$MAINPID" "$(tail -1 "$R/stopper.out" 2>/dev/null)" "stop $pid"
expect_state stopper.service inactive/dead
$L -u change stopper.service
tries=25
while [ $tries -gt 0 ] && ! in_s6rc stopper.service.stop; do tries=$((tries - 1)); sleep 0.2; done
in_s6rc stopper.service.stop && ok "started by name, its .stop oneshot follows" || bad "stopper.service.stop not brought up"
n=$(lines "$R/stopper.out")
call StopUnit ss stopper.service replace
expect_state stopper.service inactive/dead
expect_eq "StopUnit ran ExecStop= once" "$(($(lines "$R/stopper.out") - n))" "1"
call StartUnit ss stopper.service replace
expect_state stopper.service active/running
$L -d change stopcond.service
[ ! -e "$R/stopcond.out" ] && ok "no ExecStop= for a unit whose condition failed" || bad "stopcond.service ran ExecStop="

section "markers (package updates)"
# systemd-update-helper marks units at a package update and then enqueues
# them: needs-restart try-restarts, needs-reload reloads if active.
old=$(prop hello.service Service MainPID)
rm -f "$R/reload.out"
call SetUnitProperties 'sba(sv)' hello.service true 1 Markers as 1 +needs-restart
call SetUnitProperties 'sba(sv)' reloadable.service true 1 Markers as 1 +needs-reload
call SetUnitProperties 'sba(sv)' once.service true 1 Markers as 1 +needs-restart
expect_eq "Markers set" "$(prop hello.service Unit Markers)" "1 needs-restart"
call SetUnitProperties 'sba(sv)' hello.service true 1 Markers as 2 +needs-restart needs-reload 2>/dev/null \
    && bad "mixed marker syntax accepted" || ok "mixed marker syntax rejected"
call EnqueueMarkedJobs
sleep 1.5
new=$(prop hello.service Service MainPID)
[ "$old" != "$new" ] && [ "$new" != 0 ] && ok "needs-restart restarted ($old -> $new)" || bad "needs-restart: $old -> $new"
expect_eq "needs-reload ran ExecReload" "$(cat "$R/reload.out" 2>/dev/null)" "reloaded"
expect_state once.service inactive/dead
expect_eq "Markers cleared" "$(prop hello.service Unit Markers)" "0"

section "errors"
$B call $MGR StartUnit ss nope.service replace 2>&1 | grep -q "not found" && ok "unknown unit error" || bad "unknown unit error"
# Names that aren't unit names, "../../leak.service" here: the daemon used
# them as the unit's id, i.e. its state directory RUN_DIR/units/ID, and any
# caller could read files elsewhere as properties (status -> StatusText).
mkdir -p "$R/leak.service" && echo leak-secret > "$R/leak.service/status"
out=$($B call $DEST /org/freedesktop/systemd1/unit/_2e_2e_2f_2e_2e_2fleak_2eservice \
    org.freedesktop.DBus.Properties GetAll s "" 2>&1)
case "$out" in *leak-secret*) bad "unit object path left RUN_DIR: $out" ;;
    *"Unknown object"*) ok "unit object path with ../ refused" ;; *) bad "unit object path with ../: $out" ;; esac
$B call $MGR GetUnit s ../../leak.service 2>&1 | grep -q "not loaded" && ok "GetUnit refuses ../" || bad "GetUnit accepted ../"
expect_eq "ListUnitsByNames skips ../" "$($B call $MGR ListUnitsByNames as 1 ../../leak.service)" "a(ssssssouso) 0"
$B call $MGR GetUnitProcesses s ../../leak.service 2>&1 | grep -q "not loaded" && ok "GetUnitProcesses refuses ../" || bad "GetUnitProcesses accepted ../"
$B call $MGR StartUnit ss ../../leak.service replace 2>&1 | grep -q "is not valid" && ok "StartUnit refuses ../" || bad "StartUnit accepted ../"
rm -rf "$R/leak.service"
# AttachProcessesToUnit's subcgroup is joined onto the unit's cgroup: no
# ".." out of it (systemd: "not normalized").
sleep 30 &
sp=$!
$B call $MGR AttachProcessesToUnit ssau hello.service ../../x 1 $sp 2>&1 | grep -q "not normalized" \
    && ok "AttachProcessesToUnit refuses a subcgroup with .." || bad "AttachProcessesToUnit accepted a subcgroup with .."
kill $sp 2>/dev/null

section "unit files"
out=$($B call $MGR EnableUnitFiles asbb 1 forking.service false false)
expect_eq "enable without [Install]" "$out" "ba(sss) false 0"
out=$($B call $MGR DisableUnitFiles asb 1 hello.service false)
case "$out" in *unlink*) ok "disable removed links" ;; *) bad "disable: $out" ;; esac
# The s6 set decides what boots; systemctl disable only removes the links.
expect_eq "UnitFileState after disable follows the s6 set" "$(prop hello.service Unit UnitFileState)" "enabled"
out=$($B call $MGR EnableUnitFiles asbb 1 hello.service false false)
case "$out" in *symlink*) ok "enable created links" ;; *) bad "enable: $out" ;; esac
call Reload && ok "daemon-reload"
expect_state hello.service active/running
# With a live state, `sdcompat-gen reload` goes through sdcompatd (from a
# login shell s6-rc-update would label the scan directory for that shell).
out=$("$(dirname "$0")/../target/debug/sdcompat-gen" --config "$W/sdcompat.conf" reload 2>&1)
case "$out" in "reloaded by sdcompatd"*) ok "sdcompat-gen reload through sdcompatd" ;; *) bad "sdcompat-gen reload: $out" ;; esac
expect_state hello.service active/running

section "timers"
expect_state tick.timer active/waiting
sleep 6
n=$(wc -l < "$R/tick.out" 2>/dev/null || echo 0)
[ "$n" -ge 2 ] && ok "OnBootSec + OnUnitActiveSec fired $n times" || bad "tick.timer fired $n times"
n=$(wc -l < "$R/cal.out" 2>/dev/null || echo 0)
[ "$n" -ge 1 ] && ok "OnCalendar fired $n times" || bad "cal.timer fired $n times"
[ -s "$R/persist.out" ] && ok "Persistent= caught up a missed run" || bad "persist.timer did not catch up"
expect_eq "Triggers" "$(prop cal.timer Unit Triggers)" "1 cal.service"
expect_eq "TriggeredBy" "$(prop cal.service Unit TriggeredBy)" "1 cal.timer"
call StartUnit ss later.timer replace
expect_state later.timer active/waiting
next=$(prop later.timer Timer NextElapseUSecRealtime)
[ "$next" -ge 4070800000000000 ] && ok "far-future NextElapseUSecRealtime=$next" || bad "later.timer next=$next"

section "on-demand instances"
call StartUnit ss inst@a.service replace
expect_state inst@a.service active/running
expect_state prep@a.service active/exited
expect_eq "BindsTo dependency ran first" "$(head -1 "$R/prep.out" 2>/dev/null)" "up a"
$B call $MGR ListUnits | grep -q '"inst@a.service"' && ok "instance listed" || bad "instance not in ListUnits"
# s6-svlink -d leaves the instance's logger down unless started too.
[ $journal = no ] && expect_log inst@a.service "instance a up"
call StopUnit ss prep@a.service replace
expect_state inst@a.service inactive/dead
expect_state prep@a.service inactive/dead
expect_eq "ExecStop of the instance ran" "$(tail -1 "$R/prep.out")" "down a"
# logind starts user-runtime-dir@UID, then user@UID (BindsTo= it), without
# waiting: the dependency start must wait for the running job, not run
# ExecStart= a second time (/run/user/UID was mounted twice).
call StartUnit ss prep@d.service replace
call StartUnit ss inst@d.service replace
expect_state inst@d.service active/running
expect_eq "concurrent starts ran prep@d once" "$(grep -cx "up d" "$R/prep.out")" "1"
call StopUnit ss prep@d.service replace
expect_state inst@d.service inactive/dead
# While s6-svscan can't stat an entry of the scan directory (a failed
# s6-rc-update left one dangling) it aborts every scan and ends no
# supervisor: the stop must not wait for that forever (logind's stop of
# user@0 did, and every sudo after it waited for logind).
call StartUnit ss inst@h.service replace
expect_state inst@h.service active/running
scan=$(readlink -f "$R/live/scandir")
ln -s "$R/no-such-service" "$scan/dangling"
t0=$(date +%s)
call StopUnit ss prep@h.service replace
expect_state prep@h.service inactive/dead 75
t1=$(date +%s)
[ $((t1 - t0)) -lt 12 ] && ok "stop ends while s6-svscan can't scan" || bad "stop with a broken scan directory took $((t1 - t0)) s"
expect_state inst@h.service inactive/dead
grep -q "inst@h.service: s6-svscan did not release" "$W/daemon.log" && ok "the stuck release is logged" || bad "no warning for the stuck release"
# Repaired, the next scan completes and ends the supervisor left behind.
rm -f "$scan/dangling"
s6-svscanctl -an "$scan"
left() { pgrep -u "$(id -u)" -f '^s6-supervise inst@h\.service(/log)?$' >/dev/null; }
tries=25
while [ $tries -gt 0 ] && left; do tries=$((tries - 1)); sleep 0.2; done
left && bad "inst@h's supervisor still runs after the scan directory was repaired" || ok "repaired scan directory ends the leftover supervisor"

section "conflicts"
expect_state winner.service active/running
expect_state loser.service inactive/dead
call StartUnit ss loser.service replace
expect_state loser.service active/running
call RestartUnit ss winner.service replace
expect_state loser.service inactive/dead

section "Service= sockets"
call StartUnit ss vl.service replace
expect_state vl.service active/running
expect_eq "socket found via Service=" "$(cat "$R/vl.out" 2>/dev/null)" "fds=1 names=varlink"

section "logind shutdown request"
call StartUnit ss poweroff.target replace-irreversibly
sleep 0.5
[ -e "$R/poweroff-requested" ] && ok "poweroff.target ran POWEROFF_CMD" || bad "poweroff.target did nothing"

section "OnFailure= / FailureAction="
call StartUnit ss upd.service replace
sleep 2
expect_eq "OnFailure= unit ran" "$(cat "$R/cleanup.out" 2>/dev/null)" "cleaned"
[ -e "$R/reboot-requested" ] && ok "FailureAction=reboot ran REBOOT_CMD" || bad "FailureAction did nothing"

if [ $journal = no ]; then
section "s6-log"
expect_log hello.service "hello from"
else
section "journal stream"
f=$(grep -l "^sh\$" "$R"/journal/*.log 2>/dev/null | xargs grep -l "hello.service" 2>/dev/null | head -1)
if [ -n "$f" ] && head -7 "$f" | tr '\n' ' ' | grep -q "^sh hello.service 6 1 0 0 0 " && grep -q "hello from" "$f"; then
    ok "service output arrives on the journal stream with the stdout header"
else
    bad "no journal stream for hello.service: $(ls "$R/journal" | wc -l) streams"
fi
fi

section "path units"
expect_state watch.path active/waiting
touch "$R/flag"
sleep 3
expect_eq "PathExists= started the unit" "$(head -1 "$R/watch.out" 2>/dev/null)" "started"
touch "$R/chgdir/file1"
sleep 3
[ -s "$R/chg.out" ] && ok "PathChanged= started the unit" || bad "PathChanged= did nothing"
expect_eq "path Triggers" "$(prop chg.path Unit Triggers)" "1 chg.service"

section "systemd-run"
SR=""
if [ -n "${SYSTEMD_ROOT:-}" ]; then SR=$SYSTEMD_ROOT/usr/bin/systemd-run; elif command -v systemd-run >/dev/null; then SR=$(command -v systemd-run); fi
if [ -n "$SR" ]; then
    run_sr() { SYSTEMCTL=$SR "$(dirname "$0")/systemctl.sh" "$@"; }
    out=$(run_sr --unit=sr-wait --wait /bin/sh -c 'echo ran > '"$R"'/sr.out; exit 3' 2>&1); rc=$?
    case "$out" in *"status=3"*) ok "systemd-run --wait reports the exit status" ;; *) bad "systemd-run --wait: rc=$rc $out" ;; esac
    expect_eq "transient service ran" "$(cat "$R/sr.out" 2>/dev/null)" "ran"
    run_sr --unit=sr-long /bin/sleep 1000 >/dev/null 2>&1 && ok "systemd-run started sr-long" || bad "systemd-run sr-long"
    expect_state sr-long.service active/running
    expect_eq "UnitFileState transient" "$(prop sr-long.service Unit UnitFileState)" "transient"
    call StopUnit ss sr-long.service replace
    sleep 0.5
    # Like systemd, a stopped transient unit is unloaded.
    $B call $MGR GetUnit s sr-long.service >/dev/null 2>&1 && bad "sr-long still loaded" || ok "stopped transient unit unloaded"
    [ ! -e "$R/transient/sr-long.service" ] && ok "transient unit file removed" || bad "transient file left behind"
    run_sr --unit=sr-timer --on-active=1 /bin/sh -c 'echo fired > '"$R"'/srt.out' >/dev/null 2>&1 \
        && ok "systemd-run --on-active created a timer" || bad "systemd-run --on-active"
    sleep 4
    expect_eq "transient timer fired its service" "$(cat "$R/srt.out" 2>/dev/null)" "fired"
    # What the systemd RPM's file trigger runs after a package update.
    sc() { SYSTEMCTL=$(dirname "$SR")/systemctl "$(dirname "$0")/systemctl.sh" "$@"; }
    old=$(prop hello.service Service MainPID)
    sc set-property hello.service Markers=+needs-restart >/dev/null 2>&1 \
        && ok "systemctl set-property Markers" || bad "systemctl set-property Markers"
    sc enqueue-marked >/dev/null 2>&1 && ok "systemctl enqueue-marked" || bad "systemctl enqueue-marked"
    sleep 0.5
    new=$(prop hello.service Service MainPID)
    [ "$old" != "$new" ] && [ "$new" != 0 ] && ok "enqueue-marked restarted hello ($old -> $new)" || bad "enqueue-marked: $old -> $new"
    # What the file trigger for user units runs (systemd-update-helper
    # user-reload-restart): reload user@*.service, then reach each user
    # manager through systemd-run -PGq --wait ... systemd-stdio-bridge.
    call StartUnit ss notreload.service replace
    expect_state notreload.service active/running
    expect_eq "ReloadSignal property" "$(prop notreload.service Service ReloadSignal)" "10"
    expect_eq "systemctl list-units by pattern" "$(sc list-units 'notrel*' --legend=no --plain 2>&1 | cut -d' ' -f1)" "notreload.service"
    sc reload 'notrel*.service' >/dev/null 2>&1 && ok "systemctl reload with a glob" || bad "systemctl reload notrel*"
    expect_eq "notify-reload waits for READY=1" "$(cat "$R/notreload.out" 2>/dev/null)" "reloaded"
    expect_state notreload.service active/running
    expect_eq "systemd-run -PGq --wait pipes stdio" "$(echo piped | timeout 30 env SYSTEMCTL="$SR" "$(dirname "$0")/systemctl.sh" -PGq --wait /bin/cat 2>&1)" "piped"
    # stdin and stdout one socket, as sd-bus's unixexec socketpair for
    # systemctl --user -M UID@: both must reach the unit.
    out=$(SYSTEMCTL=$SR timeout 30 python3 -c '
import socket, subprocess, sys
a, b = socket.socketpair()
p = subprocess.Popen(sys.argv[1:], stdin=b, stdout=b, stderr=subprocess.DEVNULL)
b.close()
a.sendall(b"piped\n")
a.shutdown(socket.SHUT_WR)
print(a.makefile().read().strip())
p.wait()
' "$(dirname "$0")/systemctl.sh" -PGq --wait /bin/sed 's/^/got /')
    expect_eq "systemd-run -P with stdin and stdout one socket" "$out" "got piped"
    # The failed sr-wait.service stays loaded; a reload must not put it into
    # the s6-rc database (its service directory collided with the daemon's).
    call Reload && ok "daemon-reload with a failed transient unit loaded"
    expect_state hello.service active/running
    expect_state tick.timer active/waiting
else
    echo "SKIP systemd-run (set SYSTEMD_ROOT or install systemd-run)"
fi

section "failure propagation"
expect_state bad.service failed/failed
expect_state after-bad.service active/running
call StartUnit ss consumer.service replace
expect_state consumer.service active/running
out=$($B call $MGR StartUnit ss needs-bad.service replace 2>&1)
sleep 1
expect_state needs-bad.service inactive/dead
touch "$R/bad-ok"
call StartUnit ss needs-bad.service replace
expect_state needs-bad.service active/running
expect_state bad.service active/exited

section "conditions are re-checked on start"
call StartUnit ss cond2.service replace
expect_state cond2.service inactive/dead
touch "$R/cond-flag"
call StartUnit ss cond2.service replace
expect_state cond2.service active/running

section "job queue"
call StartUnit ss slow.service replace &
sleep 0.3
job=$($B call $MGR RestartUnit ss slow.service replace | cut -d'"' -f2)
jid=${job##*/}
$B get-property $DEST "$job" org.freedesktop.systemd1.Job State | grep -q waiting && ok "second job waits for the first" || bad "job not queued"
$B call $MGR CancelJob u "$jid" >/dev/null 2>&1 && ok "CancelJob on a queued job" || bad "CancelJob failed"
wait
# logind polls its stop jobs this way and lets a user go only on
# UnknownObject; NoSuchJob kept users "closing" forever.
if command -v dbus-send >/dev/null; then
    tries=30
    while [ $tries -gt 0 ]; do
        e=$(dbus-send --bus="unix:path=$R/bus" --print-reply --dest=$DEST "$job" \
            org.freedesktop.DBus.Properties.Get string:org.freedesktop.systemd1.Job string:State 2>&1)
        case $e in *Error.UnknownObject*) break ;; esac
        tries=$((tries - 1)); sleep 0.2
    done
    case $e in
        *Error.UnknownObject*) ok "finished job's object is UnknownObject" ;;
        *) bad "finished job's object: $e" ;;
    esac
else
    echo "SKIP finished job's object (no dbus-send)"
fi

section "start timeout"
call StartUnit ss extend.service replace
expect_state extend.service active/running 40
call StartUnit ss noextend.service replace
expect_state noextend.service failed/failed 40
expect_eq "start timeout result" "$(prop noextend.service Service Result)" "timeout"
call StopUnit ss extend.service replace
call ResetFailedUnit s noextend.service

section "fd store"
call StartUnit ss fdkeep.service replace
expect_state fdkeep.service active/running
n=$(lines "$R/fdkeep.out")
call RestartUnit ss fdkeep.service replace
sleep 1
expect_eq "FDSTORE=1 fd handed back after restart" "$(last_new "$R/fdkeep.out" "$n")" "start names=kept"
i1=$(stat -c %i "$R/test.sock")
call RestartUnit ss sock.service replace
sleep 1
expect_eq "listening socket kept across restart" "$(stat -c %i "$R/test.sock")" "$i1"

section "D-Bus activation"
$B emit /org/freedesktop/DBus org.freedesktop.systemd1.Activator ActivationRequest s act.service
sleep 1.5
[ ! -e "$R/act.out" ] && ok "forged ActivationRequest ignored" || bad "forged ActivationRequest started act.service"
$B --timeout=3 call org.example.Act / org.example.Act Ping >/dev/null 2>&1
sleep 1
expect_eq "real activation via dbus-daemon" "$(cat "$R/act.out" 2>/dev/null)" "activated"

section "start beside a busy s6-rc change"
L="s6-rc -l $R/live"
in_s6rc() { $L -a list | grep -qx "$1"; }
# Without this, beside1 waits for the lock, which hold1's change keeps
# until beside1 runs: a deadlock until hold1's start timeout.
$L -u change hold1.service &
hold=$!
sleep 0.5
call StartUnit ss beside1.service replace
expect_state beside1.service active/running
# JOURNAL=no: beside1.service.log is a dependency that is down, as polkit's
# logger was at boot; it has to start beside the change too.
[ $journal = no ] && expect_log beside1.service "beside1 up"
tries=50
while [ $tries -gt 0 ] && kill -0 $hold 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
if kill -0 $hold 2>/dev/null; then
    bad "the holding change did not finish"
    kill $hold
else
    ok "the holding change finished"
fi
wait $hold 2>/dev/null
tries=30
while [ $tries -gt 0 ] && ! in_s6rc beside1.service; do tries=$((tries - 1)); sleep 0.2; done
in_s6rc beside1.service && ok "beside1 recorded in s6-rc afterwards" || bad "beside1 not in s6-rc -a list"
call StopUnit ss beside1.service replace
expect_state beside1.service inactive/dead
in_s6rc beside1.service && bad "beside1 still up in s6-rc after stop" || ok "beside1 down in s6-rc after stop"
# Stopped before s6-rc knew about it: s6-rc -d change would do nothing.
$L -u change hold2.service &
hold=$!
sleep 0.5
timeout 10 $B call $MGR StartUnit ss beside2.service replace >/dev/null 2>&1
expect_state beside2.service active/running
timeout 10 $B call $MGR StopUnit ss beside2.service replace >/dev/null 2>&1
expect_state beside2.service inactive/dead
touch "$R/hold2-ready"
wait $hold
sleep 1.5
in_s6rc beside2.service && bad "beside2 recorded in s6-rc after its stop" || ok "beside2 stopped beside the change stays down"
# A target: its longrun starts beside the change, not after it.
$L -u change hold3.service &
hold=$!
sleep 0.5
timeout 10 $B call $MGR StartUnit ss beside.target replace >/dev/null 2>&1
expect_state beside3.service active/running
if kill -0 $hold 2>/dev/null && ! in_s6rc beside3.service; then
    ok "beside3 started beside the change, s6-rc not told yet"
else
    bad "beside3 not started beside the holding change"
fi
touch "$R/hold3-ready"
wait $hold
tries=30
while [ $tries -gt 0 ] && ! in_s6rc beside3.service; do tries=$((tries - 1)); sleep 0.2; done
in_s6rc beside3.service && ok "beside3 recorded in s6-rc afterwards" || bad "beside3 not in s6-rc -a list"
# -b: the daemon may be recording beside3 in s6-rc right now.
$L -b -d change hold1.service hold2.service hold3.service beside3.service

section "socket started on its own, re-activated on activity"
call StartUnit ss react.socket replace
tries=30
while [ $tries -gt 0 ] && [ ! -s "$R/react.out" ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "starting react.socket ran react.service with the FIFO" "$(cat "$R/react.out" 2>/dev/null)" "-"
expect_state react.service inactive/dead
echo a > "$R/react.fifo"
tries=30
while [ $tries -gt 0 ] && [ "$(wc -l < "$R/react.out")" -lt 2 ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "activity on the FIFO started it again" "$(sed -n 2p "$R/react.out")" "a"
expect_state react.service inactive/dead
echo b > "$R/react.fifo"
tries=30
while [ $tries -gt 0 ] && [ "$(wc -l < "$R/react.out")" -lt 3 ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "and again" "$(sed -n 3p "$R/react.out")" "b"
# An explicit start while it waits runs it now, without activity.
expect_state react.service inactive/dead
call StartUnit ss react.service replace
tries=30
while [ $tries -gt 0 ] && [ "$(wc -l < "$R/react.out")" -lt 4 ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "StartUnit while waiting runs it at once" "$(sed -n 4p "$R/react.out")" "-"
call StopUnit ss react.service replace
expect_state react.service inactive/dead
# The stop closed the held FIFO: without a reader, opening it would block.
timeout 2 sh -c 'echo c > "$1"' sh "$R/react.fifo" 2>/dev/null
sleep 2
expect_eq "stopped: no re-activation" "$(wc -l < "$R/react.out")" "4"
# Started beside a change, it exits and waits for its socket before s6-rc
# is told: recording it must not wait for readiness (systemd-rfkill was
# brought down at boot after s6-rc's timeout).
$L -u change hold4.service &
hold=$!
sleep 0.5
call StartUnit ss react2.socket replace
tries=30
while [ $tries -gt 0 ] && [ ! -s "$R/react2.out" ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "react2.service ran beside the change" "$(cat "$R/react2.out" 2>/dev/null)" "-"
expect_state react2.service inactive/dead
touch "$R/hold4-ready"
wait $hold
tries=30
while [ $tries -gt 0 ] && ! in_s6rc react2.service; do tries=$((tries - 1)); sleep 0.2; done
in_s6rc react2.service && ok "react2 recorded in s6-rc while waiting for its socket" || bad "react2 not in s6-rc -a list"
echo a > "$R/react2.fifo"
tries=30
while [ $tries -gt 0 ] && [ "$(wc -l < "$R/react2.out")" -lt 2 ]; do tries=$((tries - 1)); sleep 0.2; done
expect_eq "and still re-activated on activity" "$(sed -n 2p "$R/react2.out")" "a"
call StopUnit ss react2.service replace
expect_state react2.service inactive/dead
in_s6rc react2.service && bad "react2 still up in s6-rc after stop" || ok "react2 down in s6-rc after stop"
$L -b -d change hold4.service

section "transient scope"
sleep 1000 &
spid=$!
if call StartTransientUnit 'ssa(sv)a(sa(sv))' test-scope.scope fail 1 PIDs au 1 "$spid" 0 2>/dev/null; then
    expect_state test-scope.scope active/running
else
    echo "SKIP transient scope (needs a writable cgroup: $(cat "$W/last-error"))"
fi
kill $spid 2>/dev/null

section "units wanted by devices"
expect_state devwant.service active/running
expect_state devnotready.service inactive/dead 1
expect_state devuntagged.service inactive/dead 1
expect_eq "devices recorded" "$(cat "$R/run/devices")" "+test:dev0 devwant.service"
# A restarted daemon starts only what is new: devwant stays stopped.
call StopUnit ss devwant.service replace
expect_state devwant.service inactive/dead
touch "$W/udev/tags/systemd/+test:dev3"
printf 'E:SYSTEMD_WANTS=devlater.service\nG:systemd\n' > "$W/udev/data/+test:dev3"

section "unprivileged caller (ALLOW_UIDS empty, no polkit)"
# Swap the daemon for one with config $1; its PID goes to daemon.pid so that
# tests/sandbox.sh stop finds it.
restart_daemon() {
    kill "$(cat "$W/daemon.pid")" 2>/dev/null
    sleep 0.5
    "$DAEMON" --config "$1" daemon --address "unix:path=$R/bus" >>"$W/daemon.log" 2>&1 &
    echo $! > "$W/daemon.pid"
    sleep 1
}
grep -v '^ALLOW_UIDS=' "$W/sdcompat.conf" > "$W/unpriv.conf"
DAEMON=$(dirname "$(sed -n 's/^RUNNER=//p' "$W/sdcompat.conf")")/sdcompatd
# On-demand instances outlive the daemon: a longrun (like user@UID) with the
# oneshot it is bound to, and a oneshot alone (like systemd-backlight@).
call StartUnit ss inst@b.service replace
call StartUnit ss prep@c.service replace
call StartUnit ss prepl@d.service replace
expect_state inst@b.service active/running
expect_state prep@c.service active/exited
# And a scope (like a logind session scope), where cgroups are writable.
sleep 1000 &
kpid=$!
kscope=no
call StartTransientUnit 'ssa(sv)a(sa(sv))' keep-scope.scope fail 1 PIDs au 1 "$kpid" 0 2>/dev/null && kscope=yes
restart_daemon "$W/unpriv.conf"
expect_state inst@b.service active/running
expect_state prep@b.service active/exited
expect_state prep@c.service active/exited
$B call $MGR ListUnits | grep -q '"inst@b.service"' && ok "inst@b listed by the new daemon" || bad "inst@b not in ListUnits"
if [ $kscope = yes ]; then
    expect_state keep-scope.scope active/running
else
    echo "SKIP scope across a daemon restart (needs a writable cgroup)"
fi
denied() { $B call $MGR "$@" 2>&1 | grep -q "Access denied"; }
expect_state devlater.service active/running
expect_state devwant.service inactive/dead 1

# Root is always allowed, as with systemd: these only mean something when
# the suite runs as an ordinary user.
unpriv=yes
[ "$(id -u)" -eq 0 ] && unpriv=no
if [ $unpriv = yes ]; then
    denied StartUnit ss once.service replace && ok "StartUnit denied" || bad "StartUnit allowed"
    denied StopUnit ss hello.service replace && ok "StopUnit denied" || bad "StopUnit allowed"
    denied EnableUnitFiles asbb 1 once.service false false && ok "EnableUnitFiles denied" || bad "EnableUnitFiles allowed"
    denied Reload && ok "Reload denied" || bad "Reload allowed"
    denied SetEnvironment as 1 X=1 && ok "SetEnvironment denied" || bad "SetEnvironment allowed"
    denied KillUnit ssi hello.service all 15 && ok "KillUnit denied" || bad "KillUnit allowed"
    denied StartTransientUnit 'ssa(sv)a(sa(sv))' x.service fail 1 ExecStart 'a(sasb)' 1 /bin/true 1 /bin/true false 0 \
        && ok "StartTransientUnit denied" || bad "StartTransientUnit allowed"
    expect_state hello.service active/running
else
    echo "SKIP denied calls (running as root, which is always allowed)"
fi
$B call $MGR ListUnits >/dev/null && ok "read-only ListUnits allowed" || bad "ListUnits denied"
rm -f "$R/act.out"
$B emit /org/freedesktop/DBus org.freedesktop.systemd1.Activator ActivationRequest s act.service
sleep 1.5
expect_state act.service active/running  # still running from the real activation
[ ! -e "$R/act.out" ] && ok "forged ActivationRequest ignored for unprivileged caller" || bad "forged activation worked"
if [ -n "$SR" ] && [ $unpriv = yes ]; then
    SYSTEMCTL=$(dirname "$SR")/systemctl "$(dirname "$0")/systemctl.sh" start once.service >/dev/null 2>&1 \
        && bad "private socket accepted an unprivileged peer" || ok "private socket refuses an unprivileged peer"
fi
# Back to the normal daemon, so the sandbox stays usable.
restart_daemon "$W/sdcompat.conf"
# Stopping the oneshot stops the longrun bound to it: the daemon knows both.
call StopUnit ss prep@b.service replace
expect_state inst@b.service inactive/dead
# With sdcompat-fdstore the store is its own process: what units stored
# survived both daemon swaps. Without it (SANDBOX_FDSTORE=no) the daemon
# held them, so they went with it and the socket was bound anew.
n=$(lines "$R/fdkeep.out")
call RestartUnit ss fdkeep.service replace
sleep 1
call RestartUnit ss sock.service replace
sleep 1
if [ -e "$W/fdstore.log" ]; then
    expect_eq "FDSTORE=1 fd kept across daemon restarts" "$(last_new "$R/fdkeep.out" "$n")" "start names=kept"
    expect_eq "listening socket kept across daemon restarts" "$(stat -c %i "$R/test.sock")" "$i1"
else
    expect_eq "FDSTORE=1 fd gone with the daemon (no sdcompat-fdstore)" "$(last_new "$R/fdkeep.out" "$n")" "start names="
    [ "$(stat -c %i "$R/test.sock")" != "$i1" ] && ok "listening socket bound anew (no sdcompat-fdstore)" \
        || bad "listening socket kept without sdcompat-fdstore"
fi
expect_state prep@b.service inactive/dead
[ ! -e "$R/run/dyn/inst@b.service" ] && ok "inst@b's service directory removed" || bad "inst@b's service directory left"
if [ $kscope = yes ]; then
    # The restarted daemon still knows its cgroup: stopping it ends the process.
    call StopUnit ss keep-scope.scope replace
    tries=30
    while [ $tries -gt 0 ] && kill -0 $kpid 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
    kill -0 $kpid 2>/dev/null && bad "keep-scope's process survived StopUnit" || ok "StopUnit on a scope from an earlier daemon"
fi
kill $kpid 2>/dev/null

if [ $journal = no ]; then
    section "journal-relay"
    # The native protocol (a text and a binary field), then a stdout stream
    # with a level prefix, as systemd --user connects its units.
    python3 -c '
import socket, struct, sys, time
d = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
d.sendto(b"SYSLOG_IDENTIFIER=nat\nPRIORITY=3\nMESSAGE\n" + struct.pack("<Q", 9) + b"two\nlines\n", sys.argv[1] + "/socket")
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1] + "/stdout")
s.sendall(b"strm\nstrm.service\n6\n1\n0\n0\n0\nplain line\n<4>warned\n")
time.sleep(0.3)
' "$R/jrelay"
    tries=25
    while [ $tries -gt 0 ] && ! grep -q 'warned' "$R/devlog.out" 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
    grep -q '^<11>nat\[[0-9]*\]: two$' "$R/devlog.out" && ok "native entry forwarded" || bad "native entry not in $R/devlog.out"
    grep -q '^<14>strm\[[0-9]*\]: plain line$' "$R/devlog.out" && ok "stream line forwarded" || bad "stream line not in $R/devlog.out"
    grep -q '^<12>strm\[[0-9]*\]: warned$' "$R/devlog.out" && ok "stream level prefix" || bad "stream <4> line not in $R/devlog.out"
fi

section "shutdown stops on-demand oneshots"
# As rc.shutdown: the marker, then s6-rc takes sdcompatd down (SIGTERM).
: > "$R/run/shutting-down"
# Starts are refused at once: rc.shutdown holds the s6-rc lock, and a D-Bus
# caller would otherwise wait for the bus's activation timeout.
t0=$(date +%s)
$B --timeout=10 call org.example.Late / org.example.Late Ping >/dev/null 2>&1
t1=$(date +%s)
[ $((t1 - t0)) -lt 5 ] && ok "activation during shutdown fails at once" || bad "activation during shutdown took $((t1 - t0)) s"
if call StartUnit ss late.service replace >/dev/null; then
    bad "StartUnit accepted during shutdown"
else
    grep -q "is destructive" "$W/last-error" && ok "StartUnit refused during shutdown" || bad "StartUnit failed otherwise: $(cat "$W/last-error")"
fi
sleep 0.5
[ ! -e "$R/late.out" ] && ok "nothing started during shutdown" || bad "late.service started during shutdown"
t0=$(date +%s)
# As rc.shutdown: first stop-instances, while the daemon (and, on a real
# system, logind) still runs.
"$DAEMON" --config "$W/sdcompat.conf" stop-instances > "$W/stop-instances.out" 2>&1 &
sipid=$!
# A StopUnit for the same unit while the daemon stops it (the bus is still
# served then) waits for that stop instead of running ExecStop= again.
call StopUnit ss prep@c.service replace >/dev/null 2>&1
wait $sipid
expect_eq "stop-instances" "$?:$(cat "$W/stop-instances.out")" "0:"
t1=$(date +%s)
kill -0 "$(cat "$W/daemon.pid")" && ok "the daemon still runs after stop-instances" || bad "daemon gone after stop-instances"
# Its ExecStop= made an activation: answered while the daemon stops units.
[ $((t1 - t0)) -lt 5 ] && ok "bus answered while stopping at shutdown" || bad "prep@c's stop took $((t1 - t0)) s"
sleep 0.5
expect_eq "prep@c's ExecStop= ran once" "$(grep -cx "down c" "$R/prep.out")" "1"
grep -qx "down c" "$R/prep.out" && ok "prep@c's ExecStop= ran, started by an earlier daemon" || bad "prep@c not stopped at shutdown"
[ ! -e "$R/run/dyn-oneshot/prep@c.service" ] && ok "its marker is gone" || bad "prep@c marker left"
expect_eq "on-demand longrun's ExecStop= ran at shutdown" "$(cat "$R/prepl.out" 2>/dev/null)" "down d"
rm -f "$R/run/shutting-down"
if "$DAEMON" --config "$W/sdcompat.conf" stop-instances 2>/dev/null; then
    bad "stop-instances accepted outside a shutdown"
else
    ok "stop-instances refused outside a shutdown"
fi
# What stop-instances left (it failed, or was killed after its time limit),
# the daemon stops when s6-rc takes it down.
call StartUnit ss prep@e.service replace
expect_state prep@e.service active/exited
: > "$R/run/shutting-down"
kill "$(cat "$W/daemon.pid")"
tries=60
while [ $tries -gt 0 ] && ! grep -qx "down e" "$R/prep.out"; do tries=$((tries - 1)); sleep 0.2; done
grep -qx "down e" "$R/prep.out" && ok "the daemon's SIGTERM stops what is left" || bad "prep@e not stopped at the daemon's SIGTERM"
expect_eq "prep@c's ExecStop= not run again" "$(grep -cx "down c" "$R/prep.out")" "1"
rm -f "$R/run/shutting-down"
restart_daemon "$W/sdcompat.conf"
expect_state prep@c.service inactive/dead

section "s6-rc repository"
# The translated units are a store in an s6-rc repository; the set
# "current" decides what boots. systemd's enable state only gives a new
# service its first prescription; after that the set decides.
GEN="$(dirname "$0")/../target/debug/sdcompat-gen"
rx() { s6-rc-set-status -r "$W/repo" current "$1" 2>/dev/null | cut -d/ -f2; }
in_default() { s6-rc-db -l "$R/live" contents default 2>/dev/null | grep -qx "$1"; }
expect_eq "enabled unit starts active in the set" "$(rx hello.service)" "active"
printf '[Service]\nExecStart=/bin/sleep 1000\n' > "$W/units/repo-helper.service"
call Reload && expect_eq "a new unit not enabled starts usable" "$(rx repo-helper.service)" "usable"
cat > "$W/units/repo-new.service" <<EOF
[Unit]
Requires=repo-helper.service
After=repo-helper.service
[Service]
ExecStart=/bin/sleep 1000
[Install]
WantedBy=multi-user.target
EOF
"$GEN" --config "$W/sdcompat.conf" enable repo-new.service >/dev/null
call Reload && expect_eq "a new enabled unit starts active" "$(rx repo-new.service)" "active"
expect_eq "what an active service needs is made active (s6-rc-set-fix -u)" "$(rx repo-helper.service)" "active"
in_default repo-new.service && ok "the default bundle has it" || bad "repo-new.service not in the default bundle"
s6-rc-set-change -r "$W/repo" current usable hello.service 2>/dev/null
call Reload
expect_eq "a reload keeps the set's choice" "$(rx hello.service)" "usable"
in_default hello.service && bad "a usable service is still in the default bundle" || ok "a usable service is not in the default bundle"
expect_eq "UnitFileState of a usable service" "$(prop hello.service Unit UnitFileState)" "disabled"
# JOURNAL=no: the service's logger follows it out of the boot.
[ -n "$(rx hello.service.log)" ] && expect_eq "the logger of a usable service is usable" "$(rx hello.service.log)" "usable"
"$GEN" --config "$W/sdcompat.conf" disable repo-new.service >/dev/null
call Reload && expect_eq "systemctl disable leaves the set alone" "$(rx repo-new.service)" "active"
prev=$(readlink -f "$W/state/compiled.previous")
[ -d "$prev" ] && [ "$prev" != "$(readlink -f "$W/state/compiled")" ] && ok "the replaced database is kept as compiled.previous" || bad "compiled.previous: '$prev'"
expect_eq "older boot databases are removed" "$(find "$W/state" -maxdepth 1 -name '.current:*' | wc -l)" "2"
prop hello.service Unit UnitFileState >/dev/null
s6-rc-set-change -r "$W/repo" current active hello.service 2>/dev/null
# s6 set changes don't go through the daemon: no cached state is kept.
expect_eq "UnitFileState follows an s6 set change without a reload" "$(prop hello.service Unit UnitFileState)" "enabled"
call Reload
expect_eq "UnitFileState of an active service" "$(prop hello.service Unit UnitFileState)" "enabled"
[ -n "$(rx hello.service.log)" ] && expect_eq "the logger is active again with its service" "$(rx hello.service.log)" "active"
# A .stop oneshot follows its service, also when that was moved without -p.
s6-rc-set-change -r "$W/repo" current usable stopper.service 2>/dev/null
call Reload
expect_eq "the .stop oneshot of a usable service is usable" "$(rx stopper.service.stop)" "usable"
expect_eq "its service stays usable" "$(rx stopper.service)" "usable"
s6-rc-set-change -r "$W/repo" current active stopper.service 2>/dev/null
call Reload
expect_eq "the .stop oneshot is active again with its service" "$(rx stopper.service.stop)" "active"
in_default stopper.service.stop && ok "and in the default bundle" || bad "stopper.service.stop not in the default bundle"

section "linkwatch"
# A supervisor that started before anyone listened: its "s" went to nobody.
# A waiter named s6-rc-update (s6-ftrig-wait under that name) waits on it as
# s6_supervise_link_names() would; the sandbox's sdcompat-linkwatch
# (--after 3) sends the missed event. Under its own name it is left alone.
mkdir -p "$R/lw/svc" "$W/lw-bin"
printf '#!/bin/sh\nexec sleep 1000\n' > "$R/lw/svc/run"
chmod +x "$R/lw/svc/run"
touch "$R/lw/svc/down"
s6-supervise "$R/lw/svc" >"$W/lw-supervise.log" 2>&1 &
tries=25
while [ $tries -gt 0 ] && ! s6-svok "$R/lw/svc" 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
cp "$(command -v s6-ftrig-wait)" "$W/lw-bin/s6-rc-update"
t0=$(date +%s)
"$W/lw-bin/s6-rc-update" -t 20000 "$R/lw/svc/event" s >/dev/null 2>&1
rc=$? took=$(($(date +%s) - t0))
[ $rc = 0 ] && [ $took -lt 15 ] && ok "a missed start event is sent to a waiting s6-rc-update (${took} s)" \
    || bad "waiting s6-rc-update: exit $rc after $took s (see $W/linkwatch.log)"
s6-ftrig-wait -t 5000 "$R/lw/svc/event" s >/dev/null 2>&1 \
    && bad "linkwatch sent to a waiter that is not s6-rc-update or s6-svlink" \
    || ok "other waiters are left alone"
s6-svc -x "$R/lw/svc"

section "end state"
# A job that never finishes (logind's stop of user@0 did) holds everyone
# waiting behind it.
tries=50
while [ $tries -gt 0 ] && ! $B call $MGR ListJobs | grep -qx 'a(usssoo) 0'; do tries=$((tries - 1)); sleep 0.2; done
jobs=$($B call $MGR ListJobs)
[ "$jobs" = "a(usssoo) 0" ] && ok "no job left" || bad "jobs left at the end: $jobs"
report_damage "the end"
[ -s "$W/damage.seen" ] && bad "damage seen: $(tr '\n' ';' < "$W/damage.seen")" || ok "no damage at any section boundary"

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
