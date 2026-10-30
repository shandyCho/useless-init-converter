#!/bin/bash
# Unprivileged check of tests/hardencompare.sh: a snapshot of a made-up
# cgroup tree holding processes of this session, then compare's verdict on
# made-up snapshots. Nothing on the system is changed.
#
#   tests/hardencompare-check.sh [WORKDIR]
set -u

TOP=$(cd "$(dirname "$0")/.." && pwd)
H=$TOP/tests/hardencompare.sh
W=${1:-${TMPDIR:-/tmp}/uinitconverter-hardencompare-check}
rm -rf "$W"
mkdir -p "$W"
pass=0 fail=0
ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1"; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

echo "== snap"
# A runner-like helper started first (the oldest), the service after it.
cp "$(command -v sleep)" "$W/uinitconverter-runner"
"$W/uinitconverter-runner" 60 &
helper=$!
sleep 0.1
sleep 60 &
svc=$!
# A service only in a sub-cgroup, and one in the root cgroup (s6, not a unit).
sleep 60 &
sub=$!
cp "$(command -v sleep)" "$W/rsyslogd"
"$W/rsyslogd" 60 &
rootsvc=$!
sleep 0.1
"$W/rsyslogd" 60 &
rootsvc2=$!
C=$W/cg
mkdir -p "$C/system.slice/a.service/inner.service" "$C/system.slice/empty.service" "$C/system.slice/sub.service/udev"
: >"$C/system.slice/sub.service/cgroup.procs"
echo "$sub" >"$C/system.slice/sub.service/udev/cgroup.procs"
printf '%s\n%s\n%s\n' "$helper" "$rootsvc2" "$rootsvc" >"$C/cgroup.procs"
printf '%s\n%s\n' "$helper" "$svc" >"$C/system.slice/a.service/cgroup.procs"
echo "$helper" >"$C/system.slice/a.service/inner.service/cgroup.procs"
echo "$helper" >"$C/system.slice/empty.service/cgroup.procs"
HARDEN_TEST=1 HARDEN_INIT=s6 HARDEN_CGROUP_ROOT=$C "$H" snap "$W/snap" >"$W/snap.out" 2>&1
check "snap exits 0" '[ $? = 0 ]'
# shellcheck disable=SC2034  # read by the check strings (eval)
S=$(compgen -G "$W/snap/harden-s6-*.tsv")
check "one snapshot named after the init" '[ -n "$S" ] && [ "$(echo "$S" | wc -l)" = 1 ]'
check "main process is the service, not the helper" 'grep -qP "^unit\ta.service\tpid\t$svc$" "$S"'
check "nested and helper-only cgroups left out" '! grep -qP "\t(inner|empty).service\t" "$S"'
check "a process in a sub-cgroup is found" 'grep -qP "^unit\tsub.service\tpid\t$sub$" "$S"'
check "root cgroup: named by program, the oldest" 'grep -qP "^unit\trsyslog.service\tpid\t$rootsvc$" "$S"'
check "root cgroup: uinitconverter left out" '! grep -qP "^unit\tuinitconverter-runner.service\t" "$S"'
check "status fields" 'for f in uid nnp seccomp capbnd selinux umask; do grep -qP "^unit\ta.service\t$f\t" "$S" || exit 1; done'
check "capabilities by name" 'grep -qP "^unit\ta.service\tcapprm\tnone$" "$S"'
check "mounts per path" 'grep -qP "^unit\ta.service\tmount:/usr\t/\S* \S+ r[ow]" "$S"'
check "limits" 'grep -qP "^unit\ta.service\tlimit_nofile\t\d+/\d+$" "$S"'
kill "$helper" "$svc" "$sub" "$rootsvc" "$rootsvc2" 2>/dev/null

echo "== compare"
# FILE INIT then "UNIT FIELD VALUE" lines.
mk() {
    local f=$1 init=$2 u k v
    shift 2
    printf 'meta\tinit\t%s\nmeta\ttime\tT\nmeta\tkernel\tK\n' "$init" >"$f"
    for l in "$@"; do
        read -r u k v <<<"$l"
        printf 'unit\t%s\t%s\t%s\n' "$u" "$k" "$v" >>"$f"
    done
}
R=$W/rep
mkdir -p "$R"
mk "$R/harden-systemd-1.tsv" systemd \
    "a.service pid 10" "a.service nnp 1" "a.service seccomp 2" "a.service capbnd net_raw,sys_time" \
    "a.service ns_net private" "a.service uid 990" "a.service mount:/usr / btrfs ro" \
    "a.service mount:/home /home tmpfs ro" "a.service mount:/tmp /tmp tmpfs rw bind" \
    "a.service selinux system_u:system_r:a_t:s0" "a.service bpf_progs 2" \
    "b.service mount:/usr/lib/modules /usr/lib/modules tmpfs ro bind" "b.service seccomp_filters 5" \
    "c.service pid 13" "c.service seccomp_filters 5" "c.service mount:/dev /dev tmpfs ro bind" \
    "b.service pid 11" "b.service uid 61200" "b.service capbnd net_raw,sys_time" "b.service limit_nofile 1024/4096" \
    "only-systemd.service pid 12"
mk "$R/harden-s6-1.tsv" s6 \
    "a.service pid 20" "a.service nnp 0" "a.service seccomp 0" "a.service capbnd net_raw,sys_time,sys_admin" \
    "a.service ns_net host" "a.service uid 0" "a.service mount:/usr / btrfs rw" \
    "a.service mount:/home / btrfs rw" "a.service mount:/tmp /tmp tmpfs rw" \
    "a.service selinux system_u:system_r:unconfined_service_t:s0" "a.service bpf_progs 0" \
    "b.service mount:/usr/lib/modules /usr/lib/modules tmpfs ro" "b.service seccomp_filters 3" \
    "c.service pid 23" "c.service seccomp_filters 7" "c.service mount:/dev /dev btrfs ro" \
    "b.service pid 21" "b.service uid 62000" "b.service capbnd net_raw" "b.service limit_nofile 524288/524288" \
    "only-s6.service pid 22"
"$H" report "$R" >"$W/rep.out" 2>&1
# shellcheck disable=SC2034  # read by the check strings (eval)
rc=$?
check "exit 1 when something is weaker" '[ $rc = 1 ] && grep -q "^Result: WEAKER on s6" "$W/rep.out"'
for f in nnp seccomp capbnd ns_net uid mount:/usr mount:/home mount:/tmp selinux bpf_progs; do
    check "weaker: a.service $f" 'grep -q "^  WEAKER $f: " "$W/rep.out"'
done
check "added capabilities named" 'grep -q "^  WEAKER capbnd: .*(s6 adds sys_admin)" "$W/rep.out"'
check "fewer capabilities is not weaker" 'grep -q "^  DIFF   capbnd: .*(s6 drops sys_time)" "$W/rep.out"'
check "other settings are DIFF" 'grep -q "^  DIFF   limit_nofile: systemd=1024/4096 s6=524288/524288" "$W/rep.out"'
check "DynamicUser= ids are not compared" '! grep -q "^  .* uid: systemd=dynamic" "$W/rep.out" && ! grep -q "61200" "$W/rep.out"'
check "per-service counts" 'grep -q "^a.service: 10 weaker, 0 different" "$W/rep.out" && grep -q "^b.service: 0 weaker, 4 different" "$W/rep.out"'
check "read-only on both, only bind lost: not weaker" 'grep -q "^  DIFF   mount:/usr/lib/modules: " "$W/rep.out"'
check "read-only, other file system instead of a hidden path: weaker" 'grep -q "^  WEAKER mount:/dev: " "$W/rep.out"'
check "fewer seccomp filters: CHECK" 'grep -q "^  CHECK  seccomp_filters: systemd=5 s6=3" "$W/rep.out"'
check "more seccomp filters: DIFF" 'grep -q "^  DIFF   seccomp_filters: systemd=5 s6=7" "$W/rep.out"'
check "CHECK counted, without changing the verdict" 'grep -q "^Services with fewer seccomp filters on s6 (CHECK): 1" "$W/rep.out"'
check "services on one boot listed" 'grep -q "^Running only on systemd: only-systemd.service" "$W/rep.out" && grep -q "^Running only on s6: only-s6.service" "$W/rep.out"'
check "the pid is not compared" '! grep -q " pid: " "$W/rep.out"'

# Equal apart from a stricter s6: passes.
mk "$R/harden-s6-2.tsv" s6 "a.service pid 30" "a.service nnp 1" "a.service seccomp 2" "a.service capbnd net_raw" \
    "a.service ns_net private" "a.service uid 990" "a.service mount:/usr / btrfs ro" "a.service mount:/home /home tmpfs ro" \
    "a.service mount:/tmp /tmp tmpfs rw bind" "a.service selinux system_u:system_r:a_t:s0" "a.service bpf_progs 3"
"$H" compare "$R/harden-systemd-1.tsv" "$R/harden-s6-2.tsv" >"$W/cmp.out" 2>&1
check "exit 0 when nothing is weaker" '[ $? = 0 ] && grep -q "^Result: nothing weaker on s6" "$W/cmp.out"'
"$H" compare "$R/harden-s6-2.tsv" "$R/harden-systemd-1.tsv" >"$W/cmp.out" 2>&1
check "warns when the snapshots are swapped" 'grep -q "^warning: the first snapshot is from s6" "$W/cmp.out"'
rm "$R"/harden-s6-*.tsv
"$H" report "$R" >"$W/rep.out" 2>&1
check "incomplete without an s6 snapshot" '[ $? = 2 ] && grep -q "^Result: INCOMPLETE" "$W/rep.out"'

echo "passed: $pass  failed: $fail"
[ "$fail" = 0 ]
