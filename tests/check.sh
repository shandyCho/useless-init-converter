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
expect_eq "NFailedUnits" "$($B get-property $MGR NFailedUnits | cut -d' ' -f2)" "2"

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

echo "== transient scope"
sleep 1000 &
spid=$!
if call StartTransientUnit 'ssa(sv)a(sa(sv))' test-scope.scope fail 1 PIDs au 1 "$spid" 0 2>/dev/null; then
    expect_state test-scope.scope active/running
else
    echo "SKIP transient scope (needs a writable cgroup: $(cat "$W/last-error"))"
fi
kill $spid 2>/dev/null

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
