#!/bin/bash
# Check the running initconvert-linkwatch on the s6 boot (TESTING, "Reload
# watchdog").
#
#   sudo tests/linkwatch-check.sh
#
# Starts a throwaway s6-supervise in /run/uinitconverter/lwtest, so its start event
# has already been sent, then waits for that event under the name
# s6-rc-update: the wait a daemon-reload hangs on when a supervisor started
# before s6-rc-update listened. The system's linkwatch must send the event
# after its --after (default 10 s). A plain s6-ftrig-wait on the same
# directory must be left alone and time out. /run/uinitconverter is init_var_run_t,
# which linkwatch (init_t) may write; in /tmp the fifos would be user_tmp_t
# and the check would fail on SELinux instead. Everything it creates is
# Then a start that lost its command: `s6-svlisten -u ... s6-svc -u` on a
# second throwaway directory whose supervisor only starts afterwards (as
# s6-svscan replaces supervisors after an s6-rc-update), so nobody asked
# the service up and the wait would last until its timeout; linkwatch must
# send the s6-svc -u again. Everything it creates is
# removed at the end; no real service is touched.
set -u

usage() {
    sed -n '2,20s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

[ $# -eq 0 ] || usage
if [ "$(id -u)" != 0 ]; then
    echo "run it with sudo" >&2
    exit 2
fi

T=/run/uinitconverter/lwtest
B=$(mktemp -d)
fail=0
ok() { echo "ok:   $1"; }
bad() { echo "FAIL: $1"; fail=1; }

cleanup() {
    s6-svc -x "$T/svc" 2>/dev/null
    s6-svc -dx "$T/svc2" 2>/dev/null
    sleep 1
    rm -rf "$T" "$B"
}

if s6-svstat /run/service/uinitconverter-linkwatch 2>/dev/null | grep -q '^up '; then
    ok "initconvert-linkwatch is up"
else
    bad "initconvert-linkwatch is not up: $(s6-svstat /run/service/uinitconverter-linkwatch 2>&1)"
    exit 1
fi

trap cleanup EXIT
rm -rf "$T"
mkdir -p "$T/svc"
printf '#!/bin/sh\nexec sleep 1000\n' > "$T/svc/run"
chmod +x "$T/svc/run"
touch "$T/svc/down"
s6-supervise "$T/svc" >/dev/null 2>&1 &
tries=25
while [ $tries -gt 0 ] && ! s6-svok "$T/svc" 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
s6-svok "$T/svc" 2>/dev/null || { bad "the test supervisor did not start"; exit 1; }

cp "$(command -v s6-ftrig-wait)" "$B/s6-rc-update"
t0=$(date +%s)
"$B/s6-rc-update" -t 30000 "$T/svc/event" s >/dev/null 2>&1
rc=$? took=$(($(date +%s) - t0))
if [ $rc = 0 ] && [ $took -lt 25 ]; then
    ok "a waiting s6-rc-update got the missed start event after $took s"
else
    bad "the waiting s6-rc-update: exit $rc after $took s (see sudo ausearch -m AVC -ts recent)"
fi

t0=$(date +%s)
s6-ftrig-wait -t 15000 "$T/svc/event" s >/dev/null 2>&1
rc=$? took=$(($(date +%s) - t0))
if [ $rc != 0 ]; then
    ok "a plain s6-ftrig-wait was left alone (exit $rc after $took s)"
else
    bad "linkwatch sent an event to a waiter that is not s6-rc-update or s6-svlink"
fi

# A start whose s6-svc -u reached no supervisor.
mkdir -p "$T/svc2"
cp "$T/svc/run" "$T/svc2/run"
touch "$T/svc2/down"
s6-supervise "$T/svc2" >/dev/null 2>&1 &
tries=25
while [ $tries -gt 0 ] && ! s6-svok "$T/svc2" 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
s6-svc -x "$T/svc2"
tries=25
while [ $tries -gt 0 ] && s6-svok "$T/svc2" 2>/dev/null; do tries=$((tries - 1)); sleep 0.2; done
t0=$(date +%s)
s6-svlisten -u -t 30000 -- "$T/svc2" "" s6-svc -u -- "$T/svc2" >/dev/null 2>&1 &
lpid=$!
sleep 1
s6-supervise "$T/svc2" >/dev/null 2>&1 &
wait $lpid
rc=$? took=$(($(date +%s) - t0))
if [ $rc = 0 ] && [ $took -lt 25 ] && [ "$(s6-svstat -o up "$T/svc2" 2>/dev/null)" = true ]; then
    ok "a start that lost its s6-svc -u came up after $took s"
else
    bad "the lost start: exit $rc after $took s, $(s6-svstat "$T/svc2" 2>&1)"
fi

if grep -q "initconvert-linkwatch: .*$T/svc" /run/uncaught-logs/current; then
    ok "linkwatch logged it: $(grep -h "initconvert-linkwatch: .*$T/svc" /run/uncaught-logs/current | tail -1)"
else
    bad "no linkwatch line for $T/svc in /run/uncaught-logs/current"
fi

avc=$(ausearch -m AVC -ts recent 2>/dev/null | grep -E 'lwtest|ftrig')
[ -z "$avc" ] && ok "no SELinux denials" || bad "SELinux denials: $avc"

exit $fail
