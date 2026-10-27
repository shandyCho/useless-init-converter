#!/bin/bash
# Unprivileged check of tests/memtest.sh: install (on a systemd and on an
# s6 boot), the boot service's snapshot and report, run, uninstall, with a
# fake systemctl, uinitconverter and restorecon that only record their
# arguments; then the report's verdict on made-up snapshots. Nothing on the
# system is installed or changed.
#
#   tests/memtest-check.sh [WORKDIR]
set -u

TOP=$(cd "$(dirname "$0")/.." && pwd)
W=${1:-${TMPDIR:-/tmp}/uinitconverter-memtest-check}
rm -rf "$W"
mkdir -p "$W/bin" "$W/units"
pass=0 fail=0
ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1"; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

for c in systemctl uinitconverter restorecon; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/calls"\n' "$c" "$W" >"$W/bin/$c"
    chmod +x "$W/bin/$c"
done
export PATH="$W/bin:$PATH" MEMTEST_TEST=1
export MEMTEST_LIB=$W/lib MEMTEST_LOG_DIR=$W/log MEMTEST_UNIT_DIR=$W/units
U=$W/units/memtest-uinitconverter.service
D=$W/out
calls() { cat "$W/calls" 2>/dev/null; }
# The boot service as the unit would start it.
service() { MEMTEST_INIT=$1 sh -c "$(sed -n 's/^ExecStart=//p' "$U")" >>"$W/log/log" 2>&1; }

echo "== install on the systemd boot"
: >"$W/calls"
# An install from before the rename, under the old name.
mkdir -p "$W/units" && : >"$W/units/uinitconverter-memtest.service"
MEMTEST_INIT=systemd "$TOP/tests/memtest.sh" install --wait 0 --dir "$D" --max-diff 40 >"$W/install.out" 2>&1
check "install exits 0" '[ $? = 0 ]'
check "scripts copied" '[ -x "$W/lib/memtest.sh" ] && [ -x "$W/lib/memcompare.sh" ]'
check "unit runs the copy with the options" 'grep -qx "ExecStart=$W/lib/memtest.sh boot --wait 0 --dir $D --max-diff 40 --owner $(id -un)" "$U"'
check "unit logs to a file (no s6-log with JOURNAL=no)" 'grep -qx "StandardOutput=append:$W/log/log" "$U"'
check "unit enabled" 'calls | grep -qx "systemctl enable memtest-uinitconverter.service"'
check "old unit name disabled and removed" 'calls | grep -qx "systemctl disable uinitconverter-memtest.service" && [ ! -e "$W/units/uinitconverter-memtest.service" ]'
# The default EXCLUDE has uinitconverter*: a unit named so is never in the s6 database.
check "unit name outside the default EXCLUDE" 'case ${U##*/} in uinitconverter*) false ;; esac'
check "s6 boot database rebuilt" 'calls | grep -qx "uinitconverter reload --no-update"'

echo "== the boot service"
service systemd
check "snapshot written" '[ "$(compgen -G "$D/mem-*.tsv" | wc -l)" = 1 ]'
check "report written" 'grep -q "^Memory comparison: " "$D/report.txt"'
check "logged" 'grep -q "boot: snapshot in 0 s" "$W/log/log" && grep -q "^report: " "$W/log/log"'
# Its own processes are left out of the snapshot.
"$W/lib/memcompare.sh" snap --wait 0 --exclude-cgroup "$(sed 's/^0::\///; s/.*\///' /proc/self/cgroup)" "$W/excl" >/dev/null
check "--exclude-cgroup leaves out this cgroup" '! awk -F"\t" -v p=$$ "\$1 == \"proc\" && \$2 == p { f = 1 } END { exit !f }" "$W"/excl/*.tsv'

echo "== run, uninstall"
sleep 1  # a new file name (seconds)
MEMTEST_INIT=systemd "$TOP/tests/memtest.sh" run --wait 0 --dir "$D" >/dev/null 2>&1
check "run adds a snapshot" '[ "$(compgen -G "$D/mem-*.tsv" | wc -l)" = 2 ]'
: >"$W/calls"
MEMTEST_INIT=s6 "$TOP/tests/memtest.sh" uninstall >/dev/null 2>&1
check "uninstall removes the unit and scripts" '[ ! -e "$U" ] && [ ! -e "$W/lib" ]'
check "uninstall on s6: disable, daemon reload only" 'calls | grep -qx "systemctl disable memtest-uinitconverter.service" && calls | grep -qx "systemctl daemon-reload" && ! calls | grep -q "uinitconverter reload"'
check "snapshots kept" '[ -s "$D/report.txt" ]'
: >"$W/calls"
MEMTEST_INIT=s6 "$TOP/tests/memtest.sh" install --wait 0 --dir "$D" >/dev/null 2>&1
check "install on s6: daemon reload only" 'calls | grep -qx "systemctl daemon-reload" && ! calls | grep -q "uinitconverter reload"'
"$TOP/tests/memtest.sh" install --dir "$W/with space" >/dev/null 2>&1
check "refuses a --dir with spaces" '[ $? != 0 ]'

echo "== report"
# Made-up snapshots: MiB not available.
snapshot() { # FILE INIT UNAVAIL_MIB
    {
        printf 'meta\tinit\t%s\nmeta\tuptime_s\t600\n' "$2"
        printf 'mem\tMemTotal\t%d\nmem\tMemAvailable\t%d\n' $((8192 * 1024)) $(((8192 - $3) * 1024))
        for m in MemFree Buffers Cached SReclaimable; do printf 'mem\t%s\t0\n' "$m"; done
        printf 'proc\t1\tinit\tinit\t100\t50\t0\t%d\n' $(($3 * 1024))
    } >"$1"
}
R=$W/rep
mkdir -p "$R"
snapshot "$R/mem-systemd-1.tsv" systemd 1000
snapshot "$R/mem-s6-1.tsv" s6 1030
snapshot "$R/mem-systemd-2.tsv" systemd 1010
snapshot "$R/mem-s6-2.tsv" s6 1200   # an outlier: the median stays 1040
snapshot "$R/mem-systemd-3.tsv" systemd 990
snapshot "$R/mem-s6-3.tsv" s6 1040
"$TOP/tests/memtest.sh" report "$R" >"$W/rep.out" 2>&1
check "passes at +40 MiB with the default limit (50)" '[ $? = 0 ] && grep -q "^Result: PASS: s6 total - available +40.0 MiB" "$W/rep.out"'
check "medians: systemd 1000, s6 1040" 'grep -Eq "^  total - available +1000.0 +1040.0 +\+40.0$" "$W/rep.out"'
check "details of the latest pair" 'grep -q "^Latest pair in detail (mem-systemd-3.tsv, mem-s6-3.tsv)" "$W/rep.out"'
"$TOP/tests/memtest.sh" report "$R" --max-diff 30 >"$W/rep.out" 2>&1
check "fails with --max-diff 30" '[ $? = 1 ] && grep -q "^Result: FAIL" "$W/rep.out"'
rm "$R"/mem-s6-*.tsv
"$TOP/tests/memtest.sh" report "$R" >"$W/rep.out" 2>&1
check "incomplete without s6 snapshots" '[ $? = 2 ] && grep -q "^Result: INCOMPLETE" "$W/rep.out"'
check "a missing init shows -" 'grep -Eq "^  total - available +1000.0 +- +-$" "$W/rep.out"'

echo "passed: $pass  failed: $fail"
[ "$fail" = 0 ]
