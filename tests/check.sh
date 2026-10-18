#!/bin/sh
# Scenario checks against a running sandbox (see tests/sandbox.sh).
#   tests/check.sh WORKDIR
set -u

W=$1
R=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/sdc-test
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

echo "== boot state"
expect_state hello.service active/running
expect_state notifier.service active/running
expect_eq "notify STATUS= relayed" "$(prop notifier.service Service StatusText)" "all good"
expect_eq "alias resolves" "$(upath hello-alias.service)" "/org/freedesktop/systemd1/unit/hello_2eservice"
expect_eq "UnitFileState enabled" "$(prop hello.service Unit UnitFileState)" "enabled"
expect_eq "preset disabled" "$(prop failing.service Unit UnitFilePreset)" "disabled"

echo "== start jobs (in parallel)"
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

echo "== system state"
expect_eq "SystemState degraded" "$($B get-property $MGR SystemState | cut -d'"' -f2)" "degraded"
# failing, flappy, and bad.service (fails at boot on purpose)
expect_eq "NFailedUnits" "$($B get-property $MGR NFailedUnits | cut -d' ' -f2)" "3"

echo "== reload / restart / stop"
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

echo "== errors"
$B call $MGR StartUnit ss nope.service replace 2>&1 | grep -q "not found" && ok "unknown unit error" || bad "unknown unit error"

echo "== unit files"
out=$($B call $MGR EnableUnitFiles asbb 1 forking.service false false)
expect_eq "enable without [Install]" "$out" "ba(sss) false 0"
out=$($B call $MGR DisableUnitFiles asb 1 hello.service false)
case "$out" in *unlink*) ok "disable removed links" ;; *) bad "disable: $out" ;; esac
expect_eq "UnitFileState after disable" "$(prop hello.service Unit UnitFileState)" "disabled"
out=$($B call $MGR EnableUnitFiles asbb 1 hello.service false false)
case "$out" in *symlink*) ok "enable created links" ;; *) bad "enable: $out" ;; esac
call Reload && ok "daemon-reload"
expect_state hello.service active/running

echo "== timers"
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

echo "== on-demand instances"
call StartUnit ss inst@a.service replace
expect_state inst@a.service active/running
expect_state prep@a.service active/exited
expect_eq "BindsTo dependency ran first" "$(head -1 "$R/prep.out" 2>/dev/null)" "up a"
$B call $MGR ListUnits | grep -q '"inst@a.service"' && ok "instance listed" || bad "instance not in ListUnits"
call StopUnit ss prep@a.service replace
expect_state inst@a.service inactive/dead
expect_state prep@a.service inactive/dead
expect_eq "ExecStop of the instance ran" "$(tail -1 "$R/prep.out")" "down a"

echo "== conflicts"
expect_state winner.service active/running
expect_state loser.service inactive/dead
call StartUnit ss loser.service replace
expect_state loser.service active/running
call RestartUnit ss winner.service replace
expect_state loser.service inactive/dead

echo "== Service= sockets"
call StartUnit ss vl.service replace
expect_state vl.service active/running
expect_eq "socket found via Service=" "$(cat "$R/vl.out" 2>/dev/null)" "fds=1 names=varlink"

echo "== logind shutdown request"
call StartUnit ss poweroff.target replace-irreversibly
sleep 0.5
[ -e "$R/poweroff-requested" ] && ok "poweroff.target ran POWEROFF_CMD" || bad "poweroff.target did nothing"

echo "== OnFailure= / FailureAction="
call StartUnit ss upd.service replace
sleep 2
expect_eq "OnFailure= unit ran" "$(cat "$R/cleanup.out" 2>/dev/null)" "cleaned"
[ -e "$R/reboot-requested" ] && ok "FailureAction=reboot ran REBOOT_CMD" || bad "FailureAction did nothing"

echo "== journal stream"
f=$(grep -l "^sh\$" "$R"/journal/*.log 2>/dev/null | xargs grep -l "hello.service" 2>/dev/null | head -1)
if [ -n "$f" ] && head -7 "$f" | tr '\n' ' ' | grep -q "^sh hello.service 6 1 0 0 0 " && grep -q "hello from" "$f"; then
    ok "service output arrives on the journal stream with the stdout header"
else
    bad "no journal stream for hello.service: $(ls "$R/journal" | wc -l) streams"
fi

echo "== path units"
expect_state watch.path active/waiting
touch "$R/flag"
sleep 3
expect_eq "PathExists= started the unit" "$(head -1 "$R/watch.out" 2>/dev/null)" "started"
touch "$R/chgdir/file1"
sleep 3
[ -s "$R/chg.out" ] && ok "PathChanged= started the unit" || bad "PathChanged= did nothing"
expect_eq "path Triggers" "$(prop chg.path Unit Triggers)" "1 chg.service"

echo "== systemd-run"
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
    # The failed sr-wait.service stays loaded; a reload must not put it into
    # the s6-rc database (its service directory collided with the daemon's).
    call Reload && ok "daemon-reload with a failed transient unit loaded"
    expect_state hello.service active/running
    expect_state tick.timer active/waiting
else
    echo "SKIP systemd-run (set SYSTEMD_ROOT or install systemd-run)"
fi

echo "== failure propagation"
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

echo "== conditions are re-checked on start"
call StartUnit ss cond2.service replace
expect_state cond2.service inactive/dead
touch "$R/cond-flag"
call StartUnit ss cond2.service replace
expect_state cond2.service active/running

echo "== job queue"
call StartUnit ss slow.service replace &
sleep 0.3
job=$($B call $MGR RestartUnit ss slow.service replace | cut -d'"' -f2)
jid=${job##*/}
$B get-property $DEST "$job" org.freedesktop.systemd1.Job State | grep -q waiting && ok "second job waits for the first" || bad "job not queued"
$B call $MGR CancelJob u "$jid" >/dev/null 2>&1 && ok "CancelJob on a queued job" || bad "CancelJob failed"
wait

echo "== fd store"
call StartUnit ss fdkeep.service replace
expect_state fdkeep.service active/running
call RestartUnit ss fdkeep.service replace
sleep 1
expect_eq "FDSTORE=1 fd handed back after restart" "$(tail -1 "$R/fdkeep.out" 2>/dev/null)" "start names=kept"
i1=$(stat -c %i "$R/test.sock")
call RestartUnit ss sock.service replace
sleep 1
expect_eq "listening socket kept across restart" "$(stat -c %i "$R/test.sock")" "$i1"

echo "== D-Bus activation"
$B emit /org/freedesktop/DBus org.freedesktop.systemd1.Activator ActivationRequest s act.service
sleep 1.5
[ ! -e "$R/act.out" ] && ok "forged ActivationRequest ignored" || bad "forged ActivationRequest started act.service"
$B --timeout=3 call org.example.Act / org.example.Act Ping >/dev/null 2>&1
sleep 1
expect_eq "real activation via dbus-daemon" "$(cat "$R/act.out" 2>/dev/null)" "activated"

echo "== transient scope"
sleep 1000 &
spid=$!
if call StartTransientUnit 'ssa(sv)a(sa(sv))' test-scope.scope fail 1 PIDs au 1 "$spid" 0 2>/dev/null; then
    expect_state test-scope.scope active/running
else
    echo "SKIP transient scope (needs a writable cgroup: $(cat "$W/last-error"))"
fi
kill $spid 2>/dev/null

echo "== unprivileged caller (ALLOW_UIDS empty, no polkit)"
# Swap the daemon for one with config $1; its PID goes to daemon.pid so that
# tests/sandbox.sh stop finds it.
restart_daemon() {
    kill "$(cat "$W/daemon.pid")" 2>/dev/null
    sleep 0.5
    "$BIN" --config "$1" daemon --address "unix:path=$R/bus" >>"$W/daemon.log" 2>&1 &
    echo $! > "$W/daemon.pid"
    sleep 1
}
grep -v '^ALLOW_UIDS=' "$W/sdcompat.conf" > "$W/unpriv.conf"
BIN=$(sed -n 's/^RUNNER=//p' "$W/sdcompat.conf")
restart_daemon "$W/unpriv.conf"
denied() { $B call $MGR "$@" 2>&1 | grep -q "Access denied"; }
denied StartUnit ss once.service replace && ok "StartUnit denied" || bad "StartUnit allowed"
denied StopUnit ss hello.service replace && ok "StopUnit denied" || bad "StopUnit allowed"
denied EnableUnitFiles asbb 1 once.service false false && ok "EnableUnitFiles denied" || bad "EnableUnitFiles allowed"
denied Reload && ok "Reload denied" || bad "Reload allowed"
denied SetEnvironment as 1 X=1 && ok "SetEnvironment denied" || bad "SetEnvironment allowed"
denied KillUnit ssi hello.service all 15 && ok "KillUnit denied" || bad "KillUnit allowed"
denied StartTransientUnit 'ssa(sv)a(sa(sv))' x.service fail 1 ExecStart 'a(sasb)' 1 /bin/true 1 /bin/true false 0 \
    && ok "StartTransientUnit denied" || bad "StartTransientUnit allowed"
expect_state hello.service active/running
$B call $MGR ListUnits >/dev/null && ok "read-only ListUnits allowed" || bad "ListUnits denied"
rm -f "$R/act.out"
$B emit /org/freedesktop/DBus org.freedesktop.systemd1.Activator ActivationRequest s act.service
sleep 1.5
expect_state act.service active/running  # still running from the real activation
[ ! -e "$R/act.out" ] && ok "forged ActivationRequest ignored for unprivileged caller" || bad "forged activation worked"
if [ -n "$SR" ]; then
    SYSTEMCTL=$(dirname "$SR")/systemctl "$(dirname "$0")/systemctl.sh" start once.service >/dev/null 2>&1 \
        && bad "private socket accepted an unprivileged peer" || ok "private socket refuses an unprivileged peer"
fi
# Back to the normal daemon, so the sandbox stays usable.
restart_daemon "$W/sdcompat.conf"

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
