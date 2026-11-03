#!/bin/bash
# initconvert-user-setup in the GNOME VM of tests/gnome-vm.sh, on the normal
# systemd boot with every package installed (the uinitconverter package's
# SELinux module too) and `initconvert init` run, as on an owner's machine:
#   A  `on`, a reboot: user@1000 runs the s6 user manager, enforcing, no AVC
#   B  `off`, a reboot: no drop-in, systemd --user, USER_MANAGER=systemd
#   C  `on`, then `dnf remove uinitconverter-user`: the drop-in goes,
#      systemd --user after a reboot
#   D  a fresh VM with an older build of the same version (OLD_RPMS, from before
#      initconvert-user-setup) upgraded to CORE_RPMS: nothing turned on by
#      the upgrade, s6-rc kept, `on` works
#
#   tests/vm-user-setup.sh DIR       # DIR: the provisioned VM (gnome-vm.sh)
#
# CORE_RPMS (default DIR/rpmbuild/core/rpms) and OLD_RPMS (default: D is
# skipped) are packages/build-rpms.sh's OUTDIR/rpms. Build the old one
# first: the release holds the build time, and the newer build must have
# the newer release. For example:
#   git worktree add /tmp/old c04ebd2 && /tmp/old/packages/build-rpms.sh --test ~/uic-old
#   git worktree remove /tmp/old && packages/build-rpms.sh --test ~/uic-new
#   OLD_RPMS=~/uic-old/rpms CORE_RPMS=~/uic-new/rpms tests/vm-user-setup.sh ~/uinitconverter-vm
# STANDALONE_RPMS (default DIR/rpmbuild/standalone/rpms), SKARNET_RPMS
# (default ~/rpmbuild-skarnet/rpms). The VM goes down however this ends.
set -uo pipefail
TOP=$(cd "$(dirname "$0")/.." && pwd)
cd "$TOP" || exit 1
V=$(cd "${1:?usage: tests/vm-user-setup.sh DIR}" && pwd)
G=$TOP/tests/gnome-vm.sh
O=$(mktemp -d)
NEW=${CORE_RPMS:-$V/rpmbuild/core/rpms}
OLD=${OLD_RPMS:-}
STANDALONE=${STANDALONE_RPMS:-$V/rpmbuild/standalone/rpms}
SKARNET=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/rpms}
for d in "$NEW" "$STANDALONE" "$SKARNET" ${OLD:+"$OLD"}; do
    compgen -G "$d/*.rpm" >/dev/null || { echo "no RPMs in $d" >&2; exit 1; }
done
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
vs() { timeout "${T:-120}" "$G" ssh "$V" "$@"; }
reboot_wait() {
    local b n t=0
    b=$(vs cat /proc/sys/kernel/random/boot_id)
    T=20 vs 'sudo systemctl reboot' || true
    until n=$(T=15 vs cat /proc/sys/kernel/random/boot_id 2>/dev/null) && [ "$n" != "$b" ]; do
        t=$((t + 5)); [ $t -lt 300 ] || { bad "no new boot after 5 min"; return 1; }; sleep 5
    done
    timeout 400 "$G" wait "$V" >/dev/null || bad "no GNOME session after the reboot"
    sleep 15
}
# Read the whole output first: grep -q would stop at the first match and,
# with pipefail, the ssh it cut off (SIGPIPE) would fail the check.
status_has() { local out; out=$(vs 'sudo initconvert-user-setup status'); grep -q "$1" <<<"$out"; }
avc() { vs 'sudo ausearch -m AVC -ts boot 2>/dev/null | grep -c denied' | tr -d '[:space:]'; }
trap '"$G" stop "$V" >/dev/null 2>&1; rm -rf "$O"' EXIT

echo "=== A: full install, systemd boot, user manager on"
"$G" start "$V" >/dev/null && timeout 400 "$G" wait "$V" >/dev/null
"$G" copy "$V" "$NEW"/*.rpm "$STANDALONE"/*.rpm "$SKARNET"/*.rpm >/dev/null
T=900 vs 'sudo dnf -y -q install --nogpgcheck ./in/*.rpm || { sudo dnf clean metadata && sudo dnf -y -q install --refresh --nogpgcheck ./in/*.rpm; }' >/dev/null
T=600 vs 'sudo initconvert init --yes' >"$O/upg-init-A.log" 2>&1 || bad "initconvert init: $(tail -3 "$O/upg-init-A.log")"
vs 'sudo semodule -l | grep -qx uinitconverter' && ok "the SELinux module is loaded" || bad "no uinitconverter SELinux module"
vs 'ls -Z /usr/libexec/uinitconverter/initconvertd' | sed 's/^/   /'
vs 'sudo initconvert-user-setup on' | sed 's/^/   /'
reboot_wait
[ "$(vs cat /proc/1/comm)" = systemd ] && ok "PID 1 is systemd" || bad "PID 1: $(vs cat /proc/1/comm)"
[ "$(vs getenforce)" = Enforcing ] && ok "SELinux enforcing" || bad "SELinux: $(vs getenforce)"
vs 'sudo initconvert-user-setup status' | sed 's/^/   /'
status_has "user@1000:    the s6 user manager$" && ok "user@1000 runs the s6 user manager, no fallback" || bad "status: $(vs 'sudo initconvert-user-setup status' | grep user@1000)"
s=$(vs 'XDG_RUNTIME_DIR=/run/user/1000 systemctl --user is-system-running')
[ "$s" = running ] && ok "tester's manager: running" || bad "is-system-running: $s ($(vs 'XDG_RUNTIME_DIR=/run/user/1000 systemctl --user --failed --no-legend'))"
n=$(avc); [ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials: $(vs 'sudo ausearch -m AVC -ts boot 2>/dev/null | grep denied | head -3')"
vs 'pgrep -u tester -x gnome-shell >/dev/null' && ok "gnome-shell runs" || bad "no gnome-shell"
"$G" screenshot "$V" "$V/user-setup-A.png" >/dev/null

echo "=== B: off, then a new login"
vs 'sudo initconvert-user-setup off' | sed 's/^/   /'
reboot_wait
status_has "drop-in:      none" && ok "no drop-in" || bad "drop-in still there"
status_has "user@1000:    systemd --user" && ok "user@1000 runs systemd --user" || bad "status: $(vs 'sudo initconvert-user-setup status' | grep user@1000)"
[ "$(vs 'sed -n s/^USER_MANAGER=//p /etc/uinitconverter/uinitconverter.conf | tail -1')" = systemd ] && ok "USER_MANAGER=systemd" || bad "USER_MANAGER: $(vs 'grep USER_MANAGER /etc/uinitconverter/uinitconverter.conf')"

echo "=== C: removing uinitconverter-user removes the drop-in"
vs 'sudo initconvert-user-setup on' >/dev/null
vs 'test -e /etc/systemd/system/user@.service.d/90-uinitconverter.conf' && ok "on wrote the drop-in" || bad "no drop-in after on"
T=300 vs 'sudo dnf -y -q remove uinitconverter-user' >/dev/null
vs 'test ! -e /etc/systemd/system/user@.service.d/90-uinitconverter.conf' && ok "removal deleted the drop-in" || bad "the drop-in is still there"
vs 'systemctl cat user@1000.service | grep ^ExecStart | grep -q initconvert' && bad "user@ still names initconvert: $(vs 'systemctl cat user@1000.service | grep ^ExecStart')" || ok "user@1000.service's ExecStart is systemd --user's"
reboot_wait
[ "$(vs 'sudo readlink /proc/$(systemctl show -p MainPID --value user@1000.service)/exe | xargs basename')" = systemd ] && ok "after a reboot, user@1000 runs systemd --user" || bad "user@1000: $(vs 'sudo readlink /proc/$(systemctl show -p MainPID --value user@1000.service)/exe')"
n=$(avc); [ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
"$G" stop "$V" >/dev/null

if [ -z "$OLD" ]; then
    echo "=== D: skipped (no OLD_RPMS)"
    echo
    echo "passed: $pass  failed: $fail"
    [ "$fail" -eq 0 ]
    exit
fi
echo "=== D: upgrade from $OLD"
"$G" start "$V" >/dev/null && timeout 400 "$G" wait "$V" >/dev/null
"$G" copy "$V" "$OLD"/*.rpm "$STANDALONE"/*.rpm "$SKARNET"/*.rpm >/dev/null
# The old core RPMs apart: the new ones come into in/ later.
ver=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml)
vs "mkdir -p old && mv in/uinitconverter*-$ver-*.rpm old/"
T=900 vs 'sudo dnf -y -q install --nogpgcheck ./in/*.rpm ./old/*.rpm || { sudo dnf clean metadata && sudo dnf -y -q install --refresh --nogpgcheck ./in/*.rpm ./old/*.rpm; }' >/dev/null
T=600 vs 'sudo initconvert init --yes' >"$O/upg-init-D.log" 2>&1 || bad "old initconvert init: $(tail -3 "$O/upg-init-D.log")"
vs 'rpm -q uinitconverter-user' | sed 's/^/   old: /'
vs 'test ! -e /usr/bin/initconvert-user-setup' && ok "the old build has no initconvert-user-setup" || bad "the old build has the script"
"$G" copy "$V" "$NEW"/*.rpm >/dev/null
T=900 vs "cd in && sudo dnf -y -q upgrade --nogpgcheck ./uinitconverter*-$ver-*.rpm" >"$O/upg-dnf-D.log" 2>&1 || bad "dnf upgrade: $(tail -5 "$O/upg-dnf-D.log")"
vs 'rpm -qa "uinitconverter*" | sort' | sed 's/^/   new: /'
new_rel=$(basename "$(ls "$NEW"/uinitconverter-user-*.rpm)" .rpm)
[ "$(vs rpm -q uinitconverter-user)" = "$new_rel" ] && ok "uinitconverter-user upgraded" || bad "uinitconverter-user: $(vs rpm -q uinitconverter-user), want $new_rel"
vs 'test -x /usr/bin/initconvert-user-setup' && ok "initconvert-user-setup is installed" || bad "no initconvert-user-setup"
status_has "drop-in:      none" && ok "the upgrade alone turned nothing on" || bad "a drop-in after the upgrade"
vs 'rpm -q s6-rc >/dev/null' && ok "s6-rc stays (uinitconverter requires it)" || bad "s6-rc was removed"
T=600 vs 'sudo initconvert init --yes' >>"$O/upg-init-D.log" 2>&1 || bad "new initconvert init: $(tail -3 "$O/upg-init-D.log")"
vs 'sudo initconvert-user-setup on' >/dev/null
reboot_wait
status_has "user@1000:    the s6 user manager$" && ok "after on and a reboot, user@1000 runs the s6 user manager" || bad "status: $(vs 'sudo initconvert-user-setup status' | grep user@1000)"
n=$(avc); [ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials: $(vs 'sudo ausearch -m AVC -ts boot 2>/dev/null | grep denied | head -3')"
"$G" screenshot "$V" "$V/user-setup-D.png" >/dev/null

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
