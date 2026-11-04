#!/bin/bash
# The s6 boot in the GNOME VM of tests/gnome-vm.sh: the RPMs installed as
# the README installs them, `initconvert init --yes --default-s6`, a reboot
# into "Fedora (s6 + uinitconverter)", and checks of that boot: s6-svscan as
# PID 1, initconvertd, GDM's autologin of tester, and what the s6 boot's own
# system manager (the runner) starts for user@1000.service.
#
#   tests/vm-s6-boot.sh DIR [s6]        # USER_MANAGER=s6, uinitconverter-user installed:
#                                       # user@ runs initconvert-user-manager; then a
#                                       # reboot from the s6 boot and the same checks
#   tests/vm-s6-boot.sh DIR no-package  # USER_MANAGER=s6 without uinitconverter-user:
#                                       # user@ keeps systemd --user, the runner says why
#   tests/vm-s6-boot.sh DIR default     # every package, no USER_MANAGER=: the s6 boot's
#                                       # default, initconvert-user-manager
#   tests/vm-s6-boot.sh DIR systemd     # every package, USER_MANAGER=systemd: systemd --user
#
# RPMS: the core RPMs (packages/build-rpms.sh's OUTDIR/rpms) and the
# standalone ones, default DIR/rpmbuild/core/rpms and
# DIR/rpmbuild/standalone/rpms (the newest build of each package);
# SKARNET_RPMS: default ~/rpmbuild-skarnet/rpms (devel packages left out).
# SELINUX=permissive|enforcing (default enforcing) is init's --selinux=.
# BOOT_ONLY_S6=yes: init's --boot-only-s6=yes (uinitconverter's own initramfs,
# with mdevd; no systemd at all); the checks add that the entry boots it.
# The overlay is fresh; dnf fetches the dependencies (network). DIR keeps
# s6boot-MODE.png and s6boot-MODE-serial.log. VM_KEEP=yes keeps the VM up.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-s6-boot.sh DIR [s6|no-package|default]}
D=$(cd "$D" && pwd)
MODE=${2:-s6}
case $MODE in s6 | no-package | default | systemd) ;; *) echo "unknown mode $MODE" >&2; exit 2 ;; esac
CORE=${CORE_RPMS:-$D/rpmbuild/core/rpms}
STANDALONE=${STANDALONE_RPMS:-$D/rpmbuild/standalone/rpms}
SKARNET=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/rpms}
SELINUX=${SELINUX:-enforcing}
BOOT_ONLY_S6=${BOOT_ONLY_S6:-no}
S=$(mktemp -d)
if [ "${VM_KEEP:-no}" = yes ]; then
    trap 'cp "$D/serial.log" "$D/s6boot-$MODE-serial.log" 2>/dev/null; rm -rf "$S"' EXIT
else
    trap 'cp "$D/serial.log" "$D/s6boot-$MODE-serial.log" 2>/dev/null; "$VM" stop "$D" >/dev/null 2>&1; rm -rf "$S"' EXIT
fi

# The newest RPM of each package NAME in DIR.
newest() {
    find "$1" -maxdepth 1 -name "$2-[0-9]*.rpm" | sort | tail -1
}
mkdir "$S/rpms"
for p in uinitconverter uinitconverterd uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverter-user; do
    [ "$MODE" = no-package ] && [ $p = uinitconverter-user ] && continue
    f=$(newest "$CORE" $p)
    [ -n "$f" ] || { echo "no $p RPM in $CORE (packages/build-rpms.sh)" >&2; exit 1; }
    cp "$f" "$S/rpms/"
done
for p in uinitconverter-fdstore uinitconverter-notify uinitconverter-linkwatch uinitconverter-journal-relay; do
    f=$(newest "$STANDALONE" $p)
    [ -n "$f" ] || { echo "no $p RPM in $STANDALONE (standalone/*/build-rpm.sh)" >&2; exit 1; }
    cp "$f" "$S/rpms/"
done
for p in skalibs execline s6 s6-rc s6-linux-init s6-frontend mdevd s6-dns s6-networking; do
    f=$(newest "$SKARNET" $p)
    [ -n "$f" ] || { echo "no $p RPM in $SKARNET (dist/rpm/build-skarnet-rpms.sh)" >&2; exit 1; }
    cp "$f" "$S/rpms/"
done

cat >"$S/setup.sh" <<'EOS'
#!/bin/bash
# As root in the VM, on the systemd boot: install, configure, init.
# $1: the mode, $2: --selinux=, $3: --boot-only-s6=.
set -euo pipefail
cd ~tester/in
# Built locally, so unsigned. The base image's metadata cache can be stale
# or damaged ("Unable to validate zchunk checksums"): refreshed and once more.
dnf -y -q install --nogpgcheck ./*.rpm || { dnf clean metadata && dnf -y -q install --refresh --nogpgcheck ./*.rpm; }
rpm -qa 'uinitconverter*' | sort
mkdir -p /etc/uinitconverter
case $1 in
    s6 | no-package) echo "USER_MANAGER=s6" >>/etc/uinitconverter/uinitconverter.conf ;;
    systemd) echo "USER_MANAGER=systemd" >>/etc/uinitconverter/uinitconverter.conf ;;
esac
initconvert init --yes --default-s6 --selinux="$2" --boot-only-s6="${3:-no}"
grubby --default-title
EOS

cat >"$S/check.sh" <<'EOS'
#!/bin/bash
# As root in the VM, on the s6 boot. $1: the mode, $2: a label, $3: yes
# for --boot-only-s6=yes.
set -uo pipefail
mode=$1
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }
u=$(id -u tester)
cg=/sys/fs/cgroup/user.slice/user-$u.slice/user@$u.service
as() { runuser -u tester -- env XDG_RUNTIME_DIR="/run/user/$u" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$u/bus" "$@"; }
echo "== $2"
c=$(cat /proc/1/comm)
[ "$c" = s6-svscan ] && ok "PID 1 is s6-svscan" || bad "PID 1 is $c"
echo "   kernel: $(uname -r); cmdline: $(cat /proc/cmdline)"
if [ "${3:-no}" = yes ]; then
    i=$(grubby --info=DEFAULT 2>/dev/null | sed -n 's/^initrd="\(.*\)"/\1/p')
    case $i in *-s6.img) ok "the entry boots uinitconverter's initramfs ($i)" ;; *) bad "the entry's initrd: ${i:-none}" ;; esac
    [ -s "/boot/initramfs-$(uname -r)-s6.img" ] && ok "the image for this kernel exists" || bad "no /boot/initramfs-$(uname -r)-s6.img"
    # dracut's initramfs runs systemd as PID 1, which logs "(initrd)".
    n=$(journalctl -b --no-pager -o cat 2>/dev/null | grep -c "Running in initrd" || true)
    [ "$n" = 0 ] && ok "no systemd ran in the initramfs" || bad "systemd ran in the initramfs ($n lines)"
    findmnt -no FSTYPE / | grep -q . && ok "root is $(findmnt -no SOURCE,FSTYPE,OPTIONS / | cut -c1-120)" || bad "no root mount"
    findmnt -no SOURCE /var >/dev/null 2>&1 && ok "/var is mounted: $(findmnt -no SOURCE,FSTYPE /var)" || echo "   /var is on the root file system"
fi
pgrep -x initconvertd >/dev/null && ok "initconvertd runs" || bad "no initconvertd"
s=$(getenforce)
want=Enforcing
grep -qw enforcing=0 /proc/cmdline && want=Permissive
[ "$s" = "$want" ] && ok "SELinux is $s" || bad "SELinux is $s, the entry asked for $want"
until_ok '[ "$(systemctl is-active user@'"$u"'.service)" = active ]' 60 && ok "user@$u.service is active" || bad "user@$u.service: $(systemctl is-active "user@$u.service")"
main=$(systemctl show -p MainPID --value "user@$u.service")
exe=$(basename "$(readlink "/proc/$main/exe" 2>/dev/null)" 2>/dev/null)
echo "   user@$u.service's main process: PID $main, $exe ($(cat "/proc/$main/comm" 2>/dev/null)), $(tr '\0' ' ' <"/proc/$main/cmdline" 2>/dev/null)"
grep -qx "$main" "$cg/init.scope/cgroup.procs" 2>/dev/null && ok "it is in user@$u.service/init.scope" || bad "init.scope: $(cat "$cg/init.scope/cgroup.procs" 2>&1 | tr '\n' ' ')"
case $mode in
    s6 | default)
        [ "$exe" = initconvertd ] && ok "the runner started initconvert-user-manager" || bad "user@$u.service runs $exe"
        [ ! -e "/run/uinitconverter-user/$u" ] && ok "no fallback for tester" || bad "tester fell back: $(cat "/run/uinitconverter-user/$u")"
        own=$(as busctl --user status org.freedesktop.systemd1 2>&1 | sed -n 's/^PID=//p')
        [ "$own" = "$main" ] && ok "org.freedesktop.systemd1 on tester's bus is the s6 manager" || bad "the bus name's owner: ${own:-none}"
        [ -d "$cg/app.slice" ] && ok "the manager made app.slice" || bad "no $cg/app.slice"
        ;;
    no-package | systemd)
        [ "$exe" = systemd ] && ok "user@$u.service runs systemd --user" || bad "user@$u.service runs $exe"
        if [ "$mode" = no-package ]; then
            l=$(journalctl -b --no-pager -o cat 2>/dev/null | grep -F "install uinitconverter-user" | head -1)
            [ -n "$l" ] && ok "the runner says why: $l" || bad "no 'install uinitconverter-user' line in the journal"
        fi
        ;;
esac
# The session: on the screen, not only running.
until_ok 'pgrep -u tester -x gnome-shell >/dev/null' 120 && ok "tester's gnome-shell runs" || bad "no gnome-shell for tester after 120 s"
until_ok 's=$(loginctl show-user tester -p Display --value) && [ -n "$s" ] && [ "$(loginctl show-session "$s" -p Active --value)" = yes ]' 60 \
    && ok "tester's session is active (logind)" || bad "session: $(loginctl list-sessions --no-legend 2>&1 | tr '\n' ' ')"
sleep 15
x=$(cut -d: -f3 "/proc/$(pgrep -n -u tester -x gnome-shell)/cgroup" 2>/dev/null)
case $x in */user@$u.service/session.slice/org.gnome.Shell@user.service) ok "gnome-shell in its unit" ;; *) bad "gnome-shell's cgroup: $x" ;; esac
for x in gnome-session@gnome.target graphical-session.target org.gnome.Shell@user.service pipewire.service; do
    s=$(as systemctl --user is-active "$x" 2>&1)
    [ "$s" = active ] && ok "$x is active" || bad "$x: $s"
done
s=$(as systemctl --user is-system-running 2>&1)
[ "$s" = running ] && ok "user is-system-running: running" || bad "user is-system-running: $s"
f=$(as systemctl --user list-units --failed --no-legend 2>&1)
[ -z "$f" ] && ok "no failed user units" || bad "failed user units: $f"
echo "-- failed system units:"
systemctl --failed --no-legend --no-pager 2>&1 | sed 's/^/   /'
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep denied | sed -E 's/.*avc: +//' | sort | uniq -c | sort -rn | head -15 | cut -c1-200 | sed 's/^/   /'
echo "-- the manager's own lines (tester):"
journalctl -b "_UID=$u" --no-pager -o cat 2>/dev/null | grep -E "^(uinitconverter|initconvertd)" | cut -c1-160 | tail -20 | sed 's/^/   /'
echo "-- the runner's lines for user@:"
journalctl -b --no-pager -o cat 2>/dev/null | grep -E "^(uinitconverter\[)?user@[0-9]+\.service[]:]" | cut -c1-160 | tail -10 | sed 's/^/   /'
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

# ssh with a time limit: QEMU's port forward accepts the connection even
# when the guest's sshd isn't up (a boot stuck in stage 2), and ssh then
# waits for the greeting with no end (ConnectTimeout covers only the connect).
tssh() {
    local t=$1
    shift
    timeout "$t" "$VM" ssh "$D" "$@"
}
# Wait for SSH (not the session: the s6 boot's checks wait for that).
ssh_up() {
    local t=0
    until tssh 15 true 2>/dev/null; do
        t=$((t + 5)); [ $t -lt 300 ] || { echo "no SSH after 5 min (see $D/serial.log)" >&2; return 1; }; sleep 5
    done
}
reboot_vm() {
    local b n t=0
    b=$(tssh 15 cat /proc/sys/kernel/random/boot_id)
    tssh 30 'sudo systemctl reboot' || true
    until n=$(tssh 15 cat /proc/sys/kernel/random/boot_id 2>/dev/null) && [ "$n" != "$b" ]; do
        t=$((t + 5)); [ $t -lt 300 ] || { echo "no new boot after 5 min (see $D/serial.log)" >&2; return 1; }; sleep 5
    done
}

"$VM" start "$D"
ssh_up
"$VM" copy "$D" "$S/setup.sh" "$S/check.sh"
"$VM" copy "$D" "$S"/rpms/*
echo "== install and init (systemd boot)"
"$VM" ssh "$D" sudo bash in/setup.sh "$MODE" "$SELINUX" "$BOOT_ONLY_S6"
echo "== reboot into the s6 boot"
t0=$(date +%s)
reboot_vm
echo "   SSH up $(($(date +%s) - t0)) s after the reboot"
rc=0
"$VM" ssh "$D" sudo bash in/check.sh "$MODE" "'first s6 boot'" "$BOOT_ONLY_S6" || rc=$?
"$VM" screenshot "$D" "$D/s6boot-$MODE.png" || true
if [ "$MODE" = s6 ]; then
    echo "== reboot from the s6 boot, tester's session open"
    t0=$(date +%s)
    reboot_vm || exit 1
    echo "   SSH up $(($(date +%s) - t0)) s after the reboot"
    echo "-- the last boot's shutdown (s6 boot):"
    "$VM" ssh "$D" "sudo journalctl -b -1 --no-pager -o short-monotonic 2>/dev/null | tail -15 | cut -c1-160" | sed 's/^/   /' || true
    "$VM" ssh "$D" sudo bash in/check.sh "$MODE" "'second s6 boot'" "$BOOT_ONLY_S6" || rc=$?
fi
exit $rc
