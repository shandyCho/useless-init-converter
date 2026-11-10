#!/bin/bash
# The s6 boot in the GNOME VM of tests/gnome-vm.sh: the RPMs installed as
# the README installs them, `initconvert init --yes --default-s6`, a reboot
# into "Fedora (s6 + uinitconverter)", and checks of that boot: s6-svscan as
# PID 1, initconvertd, GDM's autologin of tester, and what the s6 boot's own
# system manager (the runner) starts for user@1000.service.
#
#   tests/vm-s6-boot.sh DIR [default]   # every package: user@ runs systemd --user; then
#                                       # a reboot from the s6 boot and the same checks
#   tests/vm-s6-boot.sh DIR old-config  # USER_MANAGER=s6 left in uinitconverter.conf (as
#                                       # 0.3.0's s6 user manager wrote it): ignored,
#                                       # user@ runs systemd --user
#   tests/vm-s6-boot.sh DIR upgrade     # 0.3.0 (OLD_RPMS) with its s6 user manager on,
#                                       # booted with s6; `dnf upgrade` to these RPMs and
#                                       # `initconvert init --yes` there, as the README
#                                       # updates; checks, then a reboot and the same checks
#   tests/vm-s6-boot.sh DIR rollback    # a new unit enabled and daemon-reload (a second
#                                       # database); a boot with uinitconverter.db=previous
#                                       # runs the first one, a boot without it the second
#
# RPMS: the core RPMs (packages/build-rpms.sh's OUTDIR/rpms) and the
# standalone ones, default DIR/rpmbuild/core/rpms and
# DIR/rpmbuild/standalone/rpms (the newest build of each package);
# SKARNET_RPMS: default ~/rpmbuild-skarnet/rpms (devel packages left out).
# OLD_RPMS (upgrade): 0.3.0's core RPMs, uinitconverter-user among them;
# default DIR/rpmbuild/core-0.3.0.
# SELINUX=permissive|enforcing (default enforcing) is init's --selinux=.
# BOOT_ONLY_S6=yes|no|auto (default auto, init's default): init's
# --boot-only-s6= (uinitconverter's own initramfs, with mdevd; no systemd at
# all). When init chose it, the checks add that the entry boots it.
# The overlay is fresh; dnf fetches the dependencies (network). DIR keeps
# s6boot-MODE.png and s6boot-MODE-serial.log. VM_KEEP=yes keeps the VM up.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-s6-boot.sh DIR [default|old-config|upgrade|rollback]}
D=$(cd "$D" && pwd)
MODE=${2:-default}
case $MODE in default | old-config | upgrade | rollback) ;; *) echo "unknown mode $MODE" >&2; exit 2 ;; esac
CORE=${CORE_RPMS:-$D/rpmbuild/core/rpms}
OLD=${OLD_RPMS:-$D/rpmbuild/core-0.3.0}
STANDALONE=${STANDALONE_RPMS:-$D/rpmbuild/standalone/rpms}
SKARNET=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/rpms}
SELINUX=${SELINUX:-enforcing}
BOOT_ONLY_S6=${BOOT_ONLY_S6:-auto}
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
mkdir "$S/rpms" "$S/new"
core=(uinitconverter uinitconverterd uinitconverter-gen uinitconverter-runner uinitconverter-timers)
for p in "${core[@]}"; do
    f=$(newest "$CORE" $p)
    [ -n "$f" ] || { echo "no $p RPM in $CORE (packages/build-rpms.sh)" >&2; exit 1; }
    if [ "$MODE" = upgrade ]; then cp "$f" "$S/new/"; else cp "$f" "$S/rpms/"; fi
done
if [ "$MODE" = upgrade ]; then
    for p in "${core[@]}" uinitconverter-user; do
        f=$(newest "$OLD" $p)
        [ -n "$f" ] || { echo "no $p RPM in $OLD (0.3.0's)" >&2; exit 1; }
        cp "$f" "$S/rpms/"
    done
fi
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
if [ "$1" = old-config ]; then
    printf 'USER_MANAGER=s6\nUSER_FALLBACK_ON=gnome-session-shutdown.target\n' >>/etc/uinitconverter/uinitconverter.conf
fi
if [ "$1" = upgrade ]; then
    initconvert-user-setup on
fi
initconvert init --yes --default-s6 --selinux="$2" --boot-only-s6="${3:-auto}"
grubby --default-title
EOS

cat >"$S/check.sh" <<'EOS'
#!/bin/bash
# As root in the VM, on the s6 boot. $1: the mode (unused), $2: a label, $3:
# init's --boot-only-s6= (the initramfs checks run when init chose it).
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
# This boot's log: the journal with JOURNAL=yes; else s6's catch-all logger,
# the per-service s6-log directories and the syslog (JOURNAL=no).
logs() { journalctl -b --no-pager -o cat 2>/dev/null; cat /run/uncaught-logs/current /var/log/uinitconverter/*/current /var/log/messages 2>/dev/null; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }
u=$(id -u tester)
cg=/sys/fs/cgroup/user.slice/user-$u.slice/user@$u.service
as() { runuser -u tester -- env XDG_RUNTIME_DIR="/run/user/$u" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$u/bus" "$@"; }
echo "== $2"
c=$(cat /proc/1/comm)
[ "$c" = s6-svscan ] && ok "PID 1 is s6-svscan" || bad "PID 1 is $c"
echo "   kernel: $(uname -r); cmdline: $(cat /proc/cmdline)"
if [ -e /etc/uinitconverter/boot-only-s6 ]; then
    ok "init chose the s6 initramfs (--boot-only-s6=${3:-auto})"
    i=$(grubby --info=DEFAULT 2>/dev/null | sed -n 's/^initrd="\(.*\)"/\1/p')
    case $i in *-s6.img) ok "the entry boots uinitconverter's initramfs ($i)" ;; *) bad "the entry's initrd: ${i:-none}" ;; esac
    [ -s "/boot/initramfs-$(uname -r)-s6.img" ] && ok "the image for this kernel exists" || bad "no /boot/initramfs-$(uname -r)-s6.img"
    # dracut's initramfs runs systemd as PID 1, which logs "(initrd)".
    n=$({ logs; dmesg 2>/dev/null; } | grep -c "Running in initrd" || true)
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
[ "$exe" = systemd ] && ok "user@$u.service runs systemd --user" || bad "user@$u.service runs $exe"
own=$(as busctl --user status org.freedesktop.systemd1 2>&1 | sed -n 's/^PID=//p')
[ "$own" = "$main" ] && ok "org.freedesktop.systemd1 on tester's bus is systemd --user" || bad "the bus name's owner: ${own:-none}"
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
# What replaces systemd-user-sessions (the tmpfiles step), -vconsole-setup and
# -backlight (rule 2): logins open, and the early steps said nothing.
[ ! -e /run/nologin ] && ok "no /run/nologin (logins open)" || bad "/run/nologin is there"
# DNS, with systemd-resolved or NetworkManager alone (RESOLVED=).
r=$(grep -E '^[[:space:]]*RESOLVED=' /etc/uinitconverter/uinitconverter.conf | tail -1)
until_ok 'getent ahosts fedoraproject.org >/dev/null' 60 && ok "DNS resolves (${r:-RESOLVED default})" || bad "no DNS (${r:-RESOLVED default}): $(cat /etc/resolv.conf 2>&1 | head -3 | tr '\n' ' ')"
s6-rc -a list | grep -qx backlight && ok "backlight is up" || bad "backlight isn't up"
# Core dumps: systemd-coredump's listener, the coredump s6 service (rule 3:
# abrt 2.17 reads systemd-coredump's journal entries, it doesn't replace it).
[ "$(cat /proc/sys/kernel/core_pattern)" = "@@/run/systemd/coredumpd/kernel" ] && ok "core_pattern is systemd-coredump's socket" || bad "core_pattern: $(cat /proc/sys/kernel/core_pattern)"
w=$(logs | grep "uinitconverter early:" | sort -u)
[ -z "$w" ] && ok "no early-boot warnings" || bad "early-boot warnings: $(echo "$w" | head -5 | tr '\n' ' ')"
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep denied | sed -E 's/.*avc: +//' | sort | uniq -c | sort -rn | head -15 | cut -c1-200 | sed 's/^/   /'
echo "-- the runner's lines for user@:"
logs | grep -E "(^|uinitconverter\[|: )user@[0-9]+\.service[]:]" | cut -c1-160 | tail -10 | sed 's/^/   /'
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/upgrade.sh" <<'EOS'
#!/bin/bash
# As root in the VM, on 0.3.0's s6 boot (mode upgrade). $1...: the new core
# RPMs' file names in ~tester/in. The README's update: dnf upgrade, init.
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
# This boot's log: the journal with JOURNAL=yes; else s6's catch-all logger,
# the per-service s6-log directories and the syslog (JOURNAL=no).
logs() { journalctl -b --no-pager -o cat 2>/dev/null; cat /run/uncaught-logs/current /var/log/uinitconverter/*/current /var/log/messages 2>/dev/null; }
u=$(id -u tester)
cd ~tester/in
echo "== 0.3.0's s6 boot"
c=$(cat /proc/1/comm)
[ "$c" = s6-svscan ] && ok "PID 1 is s6-svscan" || bad "PID 1 is $c"
rpm -q uinitconverter-user >/dev/null && ok "uinitconverter-user is installed" || bad "no uinitconverter-user"
main=$(systemctl show -p MainPID --value "user@$u.service")
echo "   user@$u.service: $(systemctl is-active "user@$u.service"), PID $main: $(tr '\0' ' ' <"/proc/$main/cmdline" 2>/dev/null)"
[ -e /etc/systemd/system/user@.service.d/90-uinitconverter.conf ] && ok "initconvert-user-setup's drop-in is there" || bad "no drop-in"
[ -e /run/s6-rc/servicedirs/uinitconverter-units ] && bad "0.3.0's database has the holder" || ok "0.3.0's database has no holder"
# The daemon's state: its process, s6's view, its bus name, the live
# database (all with time limits: a daemon off the bus hangs systemctl).
daemon() {
    echo "   initconvertd: $(pgrep -x initconvertd | tr '\n' ' ')/ $(s6-svstat /run/service/uinitconverterd 2>&1)"
    echo "   bus name: $(timeout 10 busctl status org.freedesktop.systemd1 2>&1 | sed -n 's/^PID=//p;s/^Failed.*/&/p' | head -1)"
    echo "   live database: $(readlink -f /run/s6-rc/compiled); holder: $([ -e /run/s6-rc/servicedirs/uinitconverter-units ] && echo yes || echo no)"
}
echo "-- before dnf:"
daemon
# A daemon off the bus takes SSH with it (logins wait on it): what runs then
# goes to the serial console (DIR/s6boot-upgrade-serial.log).
setsid sh -c 'for t in 20 60; do sleep 20; {
    echo "=== upgrade watch ${t}s"
    ps -eo pid,ppid,stat,etime,args | grep -E "initconvert|s6-rc|s6-svc|dnf|systemctl" | grep -v grep | cut -c1-240
    for d in /run/service/*/; do n=$(basename "$d"); s=$(s6-svstat "$d" 2>/dev/null); case $s in *"want down"*|down*) echo "$n: $s" ;; esac; done
    echo "=== end"; } >/dev/ttyS0 2>&1; done' </dev/null >/dev/null 2>&1 &
t0=$(date +%s)
echo "== dnf upgrade to the new RPMs"
timeout 900 dnf -y -q upgrade --nogpgcheck "$@" 2>&1 | tail -20 | sed 's/^/   /'
echo "   dnf took $(($(date +%s) - t0)) s"
rpm -qa 'uinitconverter*' | sort | sed 's/^/   /'
rpm -q uinitconverter-user >/dev/null && bad "uinitconverter-user is still installed" || ok "uinitconverter-user is gone (Obsoletes)"
[ -e /etc/systemd/system/user@.service.d/90-uinitconverter.conf ] && bad "the drop-in is still there" || ok "the drop-in is gone"
# RESOLVED's default became no: the update keeps this install's resolved.
grep -q '^RESOLVED=yes' /etc/uinitconverter/uinitconverter.conf && ok "the update kept RESOLVED=yes" || bad "no RESOLVED=yes after the update"
echo "-- right after dnf, before init:"
daemon
# The 0.3.0 package's %preun reloaded (the drop-in was there): the daemon
# must have stayed up and on the bus through it.
o=$(timeout 10 busctl status org.freedesktop.systemd1 2>/dev/null | sed -n 's/^PID=//p')
[ -n "$o" ] && [ "$o" = "$(pgrep -x initconvertd)" ] && ok "initconvertd stayed on the bus" || bad "initconvertd off the bus: $(s6-svstat /run/service/uinitconverterd 2>&1)"
for m in syslog journal-relay; do
    [ -e /etc/uinitconverter/s6/uinitconverter-sysinit/contents.d/$m ] && bad "the update put $m back into uinitconverter-sysinit" || ok "$m isn't back in uinitconverter-sysinit"
done
echo "   is-system-running: $(timeout 30 systemctl is-system-running 2>&1)"
echo "-- initconvertd's lines since dnf:"
timeout 30 journalctl -b --no-pager -o short-monotonic --since "@$t0" 2>/dev/null | grep -E "initconvert|s6-rc" | tail -40 | cut -c1-220 | sed 's/^/   /'
echo "== initconvert init --yes"
timeout 600 initconvert init --yes 2>&1 | tail -15 | sed 's/^/   /'
echo "   init: exit ${PIPESTATUS[0]}"
daemon
s=$(timeout 30 systemctl is-system-running 2>&1)
case $s in running | degraded) ok "is-system-running: $s" ;; *) bad "is-system-running: $s" ;; esac
echo "   holder in the live database: $([ -e /run/s6-rc/servicedirs/uinitconverter-units ] && echo yes || echo no)"
s=$(systemctl is-active sshd.service)
[ "$s" = active ] && ok "sshd.service is active" || bad "sshd.service: $s"
s=$(systemctl show -p NeedDaemonReload --value sshd.service)
[ "$s" = no ] && ok "NeedDaemonReload=no" || bad "NeedDaemonReload=$s"
s=$(systemctl is-active "user@$u.service")
[ "$s" = active ] && ok "user@$u.service stays active until the reboot" || bad "user@$u.service: $s"
echo "-- failed system units:"
systemctl --failed --no-legend --no-pager 2>&1 | sed 's/^/   /'
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/rollback.sh" <<'EOS'
#!/bin/bash
# As root in the VM, on the s6 boot (mode rollback). $1: step.
#   new:      enable rbtest.service, daemon-reload (a second database), and
#             boot uinitconverter.db=previous next
#   previous: on that boot: the first database, its units and its index
#   current:  after a boot without it: the second database again
#   essential: a down service put into the bundle dbus depends on, then a
#             reload: installing it live would restart dbus and initconvertd,
#             so it is installed for the next boot only; both keep running
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
# This boot's log: the journal with JOURNAL=yes; else s6's catch-all logger,
# the per-service s6-log directories and the syslog (JOURNAL=no).
logs() { journalctl -b --no-pager -o cat 2>/dev/null; cat /run/uncaught-logs/current /var/log/uinitconverter/*/current /var/log/messages 2>/dev/null; }
H=/run/s6-rc/servicedirs/uinitconverter-units/data
live() { readlink -f /run/s6-rc/compiled; }
in_index() { grep -q rbtest "$H/index" 2>/dev/null; }
case $1 in
new)
    echo "== a second database: rbtest.service enabled"
    l0=$(live)
    in_index && bad "rbtest is in the first database" || ok "rbtest isn't in the first database"
    printf '[Unit]\nDescription=rollback test\n[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target\n' >/etc/systemd/system/rbtest.service
    # One reload, one new database: `systemctl enable` reloads by itself.
    systemctl enable --no-reload rbtest.service 2>&1 | sed 's/^/   /'
    systemctl daemon-reload
    l1=$(live)
    [ "$l1" != "$l0" ] && ok "daemon-reload installed another database ($l0 -> $l1)" || bad "the live database is still $l0"
    in_index && ok "rbtest is in the new database's index" || bad "rbtest isn't in the new index"
    systemctl start rbtest.service
    s=$(systemctl is-active rbtest.service)
    [ "$s" = active ] && ok "rbtest.service is active" || bad "rbtest.service: $s"
    p=$(readlink -f /var/lib/uinitconverter/compiled.previous)
    [ "$p" = "$l0" ] && ok "compiled.previous is the first database" || bad "compiled.previous: $p, the first: $l0"
    echo "$l0" >/root/rb-first; echo "$l1" >/root/rb-second
    grubby --update-kernel=DEFAULT --args=uinitconverter.db=previous
    ;;
previous)
    echo "== the rollback boot"
    grep -qw uinitconverter.db=previous /proc/cmdline && ok "booted with uinitconverter.db=previous" || bad "cmdline: $(cat /proc/cmdline)"
    l=$(live)
    [ "$l" = "$(cat /root/rb-first)" ] && ok "the live database is the first one" || bad "the live database: $l, the first: $(cat /root/rb-first)"
    [ -e "$H/index" ] && ok "it has the holder" || bad "no holder"
    in_index && bad "rbtest is in the live index" || ok "rbtest isn't in the live index"
    s6-rc -a list | grep -q rbtest && bad "s6-rc has rbtest up" || ok "s6-rc has no rbtest"
    s=$(systemctl is-active rbtest.service)
    [ "$s" = inactive ] && ok "rbtest.service is inactive (not in this database)" || bad "rbtest.service: $s"
    s=$(systemctl show -p NeedDaemonReload --value sshd.service)
    [ "$s" = no ] && ok "sshd.service: NeedDaemonReload=no" || bad "sshd.service: NeedDaemonReload=$s"
    grubby --update-kernel=DEFAULT --remove-args=uinitconverter.db=previous
    ;;
current)
    echo "== a boot without uinitconverter.db=previous"
    grep -qw uinitconverter.db=previous /proc/cmdline && bad "still booted with uinitconverter.db=previous" || ok "booted without uinitconverter.db=previous"
    l=$(live)
    [ "$l" = "$(cat /root/rb-second)" ] && ok "the live database is the second one" || bad "the live database: $l, the second: $(cat /root/rb-second)"
    in_index && ok "rbtest is in the live index" || bad "rbtest isn't in the live index"
    s=$(systemctl is-active rbtest.service)
    [ "$s" = active ] && ok "rbtest.service is active (enabled in this database)" || bad "rbtest.service: $s"
    ;;
essential)
    echo "== a reload that would restart the essential services"
    # A new s6 service (in /etc/s6/sources, EXTRA_SOURCE) put into the
    # bundle: dbus and initconvertd would gain a dependency on a service
    # that wasn't up.
    m=/etc/uinitconverter/s6/uinitconverter-sysinit/contents.d/vmtest-down
    v=/etc/s6/sources/vmtest-down
    mkdir -p "$v"
    echo oneshot >"$v/type"
    echo true >"$v/up"
    d0=$(pgrep -x initconvertd); b0=$(s6-svstat -p /run/service/dbus); l0=$(live)
    touch "$m"
    timeout 180 systemctl daemon-reload; r=$?
    rm -f "$m"; rm -rf "$v"
    [ "$r" = 0 ] && ok "daemon-reload returned" || bad "daemon-reload: exit $r"
    [ "$(pgrep -x initconvertd)" = "$d0" ] && ok "initconvertd kept running (PID $d0)" || bad "initconvertd: $d0 -> $(pgrep -x initconvertd)"
    [ "$(s6-svstat -p /run/service/dbus)" = "$b0" ] && ok "dbus kept running" || bad "dbus: $b0 -> $(s6-svstat -p /run/service/dbus)"
    o=$(timeout 10 busctl status org.freedesktop.systemd1 2>/dev/null | sed -n 's/^PID=//p')
    [ "$o" = "$d0" ] && ok "initconvertd is on the bus" || bad "org.freedesktop.systemd1: ${o:-no owner}"
    [ "$(live)" = "$l0" ] && ok "the live database stayed" || bad "the live database changed: $(live)"
    b=$(readlink -f /var/lib/uinitconverter/compiled)
    [ "$b" != "$l0" ] && ok "the next boot's database is the new one" || bad "the boot database is still the live one"
    # The daemon's line reaches s6's catch-all logger a moment later.
    t=0
    until logs | grep "not applied to the running system" >/dev/null || [ $t -ge 10 ]; do sleep 1; t=$((t + 1)); done
    logs | grep "not applied to the running system" >/dev/null \
        && ok "the daemon says why" || bad "no 'not applied to the running system' line"
    timeout 180 systemctl daemon-reload && ok "a reload without it" || bad "the reload without it failed"
    [ "$(live)" = "$(readlink -f /var/lib/uinitconverter/compiled)" ] && ok "applied live again" || bad "live $(live), boot $(readlink -f /var/lib/uinitconverter/compiled)"
    s=$(timeout 30 systemctl is-system-running 2>&1)
    case $s in running | degraded) ok "is-system-running: $s" ;; *) bad "is-system-running: $s" ;; esac
    ;;
esac
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
"$VM" copy "$D" "$S/setup.sh" "$S/check.sh" "$S/upgrade.sh" "$S/rollback.sh"
"$VM" copy "$D" "$S"/rpms/*
echo "== install and init (systemd boot)"
"$VM" ssh "$D" sudo bash in/setup.sh "$MODE" "$SELINUX" "$BOOT_ONLY_S6"
echo "== reboot into the s6 boot"
t0=$(date +%s)
reboot_vm
echo "   SSH up $(($(date +%s) - t0)) s after the reboot"
rc=0
if [ "$MODE" = upgrade ]; then
    "$VM" copy "$D" "$S"/new/*
    new=()
    for f in "$S"/new/*; do new+=("./$(basename "$f")"); done
    timeout 2400 "$VM" ssh "$D" sudo bash in/upgrade.sh "${new[@]}" || rc=$?
    # Still booted by 0.3.0's rc.init (the live state on the boot link): a
    # deferred install must pin it first.
    timeout 600 "$VM" ssh "$D" sudo bash in/rollback.sh essential || rc=$?
    echo "== reboot after the upgrade"
    reboot_vm || exit 1
fi
"$VM" ssh "$D" sudo bash in/check.sh "$MODE" "'first s6 boot'" "$BOOT_ONLY_S6" || rc=$?
"$VM" screenshot "$D" "$D/s6boot-$MODE.png" || true
if [ "$MODE" = upgrade ]; then
    "$VM" ssh "$D" "sudo test -e /run/s6-rc/servicedirs/uinitconverter-units" &&
        echo "PASS the live database has the holder" || { echo "FAIL no holder after the reboot"; rc=1; }
fi
if [ "$MODE" = rollback ]; then
    "$VM" ssh "$D" sudo bash in/rollback.sh new || rc=$?
    echo "== reboot with uinitconverter.db=previous"
    reboot_vm || exit 1
    "$VM" ssh "$D" sudo bash in/rollback.sh previous || rc=$?
    "$VM" ssh "$D" sudo bash in/check.sh "$MODE" "'rollback boot'" "$BOOT_ONLY_S6" || rc=$?
    echo "== reboot without it"
    reboot_vm || exit 1
    "$VM" ssh "$D" sudo bash in/rollback.sh current || rc=$?
    timeout 600 "$VM" ssh "$D" sudo bash in/rollback.sh essential || rc=$?
fi
if [ "$MODE" = default ]; then
    echo "== reboot from the s6 boot, tester's session open"
    t0=$(date +%s)
    reboot_vm || exit 1
    echo "   SSH up $(($(date +%s) - t0)) s after the reboot"
    echo "-- the last boot's shutdown (s6 boot):"
    "$VM" ssh "$D" "sudo journalctl -b -1 --no-pager -o short-monotonic 2>/dev/null | tail -15 | cut -c1-160" | sed 's/^/   /' || true
    "$VM" ssh "$D" sudo bash in/check.sh "$MODE" "'second s6 boot'" "$BOOT_ONLY_S6" || rc=$?
fi
exit $rc
