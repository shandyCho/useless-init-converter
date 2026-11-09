#!/bin/bash
# initconvert-user-setup (dist/user/) under UINITCONVERTER_SETUP_ROOT,
# unprivileged: on/off write and remove the drop-in and USER_MANAGER=, and
# status reads a fake /sys/fs/cgroup and /proc there:
#   - the systemd boot: user@'s only process is the manager;
#   - the s6 boot: initconvert-runner first, the manager its child
#     (status once named the runner there);
#   - a manager that fell back to systemd --user, with its record;
#   - a record in the user's name but owned by someone else, which
#     initconvertd ignores and status must too;
#   - `on` warns that the s6 manager is not recommended, and an unset
#     USER_MANAGER= reads as systemd, also on the s6 boot.
#
#   tests/user-setup.sh WORKDIR
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
SETUP=$TOP/dist/user/initconvert-user-setup
W=${1:?usage: tests/user-setup.sh WORKDIR}
rm -rf "$W"
mkdir -p "$W"
W=$(cd "$W" && pwd)
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
run() { UINITCONVERTER_SETUP_ROOT=$W sh "$SETUP" "$@"; }
me=$(id -u)
other=$((me + 1))

# A user@ cgroup for UID $1 whose init.scope holds fake PIDs with the
# executables named in the rest of the arguments, in that order.
pid=100
user_at() {
    local uid=$1 cg
    shift
    cg=$W/sys/fs/cgroup/user.slice/user-$uid.slice/user@$uid.service/init.scope
    mkdir -p "$cg"
    : >"$cg/cgroup.procs"
    for exe in "$@"; do
        pid=$((pid + 1))
        mkdir -p "$W/proc/$pid"
        ln -s "$exe" "$W/proc/$pid/exe"
        echo "$pid" >>"$cg/cgroup.procs"
    done
}
line() { grep "^user@$1:" <<<"$out" || true; }

mkdir -p "$W/usr/libexec/uinitconverter" "$W/etc/uinitconverter"
: >"$W/usr/libexec/uinitconverter/initconvertd"
ln -s initconvertd "$W/usr/libexec/uinitconverter/initconvert-user-manager"

# Unset, on the s6 boot (PID 1 s6-svscan): systemd, the default on both.
mkdir -p "$W/proc/1"
echo s6-svscan >"$W/proc/1/comm"
out=$(run status)
grep -qx "USER_MANAGER: unset (systemd)" <<<"$out" && ok "unset: systemd, also on the s6 boot" || bad "unset: $out"

err=$(run on 2>&1 >/dev/null)
grep -q "^Not recommended:" <<<"$err" && ok "on: warns that it is not recommended" || bad "on: no warning: $err"
[ -f "$W/etc/systemd/system/user@.service.d/90-uinitconverter.conf" ] && ok "on: drop-in written" || bad "on: no drop-in"
grep -qx USER_MANAGER=s6 "$W/etc/uinitconverter/uinitconverter.conf" && ok "on: USER_MANAGER=s6" || bad "on: $(cat "$W/etc/uinitconverter/uinitconverter.conf")"

# The systemd boot: the manager alone (exe is the link's target).
user_at 10 /usr/libexec/uinitconverter/initconvertd
user_at 11 /usr/lib/systemd/systemd
# The s6 boot: the runner, its notify helper, then the manager.
user_at 12 /usr/libexec/uinitconverter/initconvert-runner /usr/libexec/uinitconverter/initconvert-runner /usr/libexec/uinitconverter/initconvertd
# The s6 boot after a package update replaced initconvertd.
user_at 13 /usr/libexec/uinitconverter/initconvert-runner "/usr/libexec/uinitconverter/initconvertd (deleted)"
# Fell back on the s6 boot: the manager's process exec'd systemd --user.
user_at "$me" /usr/libexec/uinitconverter/initconvert-runner /usr/lib/systemd/systemd
mkdir -p "$W/run/uinitconverter-user"
echo "the start of gnome-session@gnome.target ended dependency" >"$W/run/uinitconverter-user/$me"
# An empty init.scope (a stopped manager's cgroup) is left out.
user_at 14
# A record named after another user but owned by us: not theirs.
user_at "$other" /usr/libexec/uinitconverter/initconvert-runner /usr/libexec/uinitconverter/initconvertd
echo "forged" >"$W/run/uinitconverter-user/$other"

out=$(run status)
[ "$(line 10)" = "user@10:    the s6 user manager" ] && ok "systemd boot: the s6 user manager" || bad "systemd boot: $(line 10)"
[ "$(line 11)" = "user@11:    systemd --user" ] && ok "systemd boot: systemd --user" || bad "systemd boot: $(line 11)"
[ "$(line 12)" = "user@12:    the s6 user manager" ] && ok "s6 boot: the manager behind the runner" || bad "s6 boot: $(line 12)"
[ "$(line 13)" = "user@13:    the s6 user manager" ] && ok "s6 boot: a replaced (deleted) initconvertd" || bad "deleted: $(line 13)"
[ "$(line "$me")" = "user@$me:    systemd --user (fell back: the start of gnome-session@gnome.target ended dependency)" ] && ok "fallback with its reason" || bad "fallback: $(line "$me")"
[ -z "$(line 14)" ] && ok "empty init.scope left out" || bad "empty: $(line 14)"
[ "$(line "$other")" = "user@$other:    the s6 user manager" ] && ok "another user's file is no record" || bad "forged: $(line "$other")"

# The record in the user's runtime directory (FALLBACK_DIR's name taken).
rm "$W/run/uinitconverter-user/$me"
mkdir -p "$W/run/user/$me/uinitconverter-fallback"
echo "crash" >"$W/run/user/$me/uinitconverter-fallback/$me"
out=$(run status)
[ "$(line "$me")" = "user@$me:    systemd --user (fell back: crash)" ] && ok "record in the runtime directory" || bad "runtime record: $(line "$me")"

mkdir -p "$W/proc"
echo "BOOT_IMAGE=/vmlinuz uinitconverter.user_manager=systemd quiet" >"$W/proc/cmdline"
out=$(run status)
grep -qx "this boot:    uinitconverter.user_manager=systemd on the kernel command line" <<<"$out" && ok "kernel flag shown" || bad "kernel flag: $out"

run off >/dev/null
[ ! -e "$W/etc/systemd/system/user@.service.d" ] && ok "off: drop-in and its directory removed" || bad "off: drop-in left"
grep -qx USER_MANAGER=systemd "$W/etc/uinitconverter/uinitconverter.conf" && ok "off: USER_MANAGER=systemd" || bad "off: $(cat "$W/etc/uinitconverter/uinitconverter.conf")"

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
