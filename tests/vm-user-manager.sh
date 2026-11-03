#!/bin/bash
# The s6 user manager under a real user@UID.service (slice A, work_history
# part 141), in the GNOME VM of tests/gnome-vm.sh. The VM boots systemd; a
# drop-in makes one test user's user@ run `uinitconverterd user-manager`,
# and uinitconverter.user_manager=s6 on the kernel command line lets it
# (the launcher hands over to systemd --user on a systemd boot otherwise).
# GDM, the greeter and tester's GNOME session keep systemd --user. A second
# user with systemd --user is the memory comparison.
#
#   tests/vm-user-manager.sh DIR [user]  # DIR: the provisioned VM (gnome-vm.sh)
#   tests/vm-user-manager.sh DIR greeter # every user@ (GDM's greeter too) on the s6
#                                        # manager, autologin off: the greeter must
#                                        # show (DIR/greeter-greeter.png)
#   tests/vm-user-manager.sh DIR baseline # the same greeter checks on stock systemd
#                                        # --user, for comparison (DIR/greeter-baseline.png)
#   tests/vm-user-manager.sh DIR session # every user@ on the s6 manager, GDM autologin
#                                        # as tester: gnome-vm.sh wait/check pass, the
#                                        # session's units run, and an app started from
#                                        # GNOME Shell lands in its app-*.scope
#                                        # (DIR/session-session.png)
#   tests/vm-user-manager.sh DIR session-baseline # the same on systemd --user
#   tests/vm-user-manager.sh DIR apps    # the session as above, then slice D: user
#                                        # timers, every user socket (Accept=yes
#                                        # too), systemd-oomd watching the user
#                                        # slices, a podman quadlet container, and a
#                                        # flatpak app (installed for tester from
#                                        # the fedora remote: ~3 GB, network) in
#                                        # its app-flatpak-*.scope
#   tests/vm-user-manager.sh DIR apps-baseline # the same on systemd --user
#
# Copies target/debug's uinitconverterd, -runner and -timers (cargo build
# first) and the skalibs and s6 RPMs from SKARNET_RPMS (default
# ~/rpmbuild-skarnet/RPMS/x86_64, the newest of each) into a fresh overlay.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-user-manager.sh DIR [user|greeter|baseline|session|session-baseline|apps|apps-baseline]}
MODE=${2:-user}
case $MODE in user | greeter | baseline | session | session-baseline | apps | apps-baseline) ;; *) echo "unknown mode $MODE" >&2; exit 2 ;; esac
RPMS=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/RPMS/x86_64}
S=$(mktemp -d)
# The VM goes down however this ends (VM_KEEP=yes: it stays up, for
# `gnome-vm.sh ssh DIR`).
if [ "${VM_KEEP:-no}" = yes ]; then
    trap 'rm -rf "$S"' EXIT
else
    trap '"$VM" stop "$D" >/dev/null 2>&1; rm -rf "$S"' EXIT
fi

pick() { find "$RPMS" -maxdepth 1 -name "$1-[0-9]*.x86_64.rpm" | sort | tail -1; }
skalibs=$(pick skalibs)
execline=$(pick execline)
s6=$(pick s6)
[ -n "$skalibs" ] && [ -n "$execline" ] && [ -n "$s6" ] || { echo "no skalibs/execline/s6 RPMs in $RPMS" >&2; exit 1; }
# BUILD=release for the memory figure (debug binaries are several times
# larger): cargo build --release first.
BUILD=${BUILD:-debug}
for b in uinitconverterd uinitconverter-runner uinitconverter-timers; do
    [ -x "$TOP/target/$BUILD/$b" ] || { echo "build first: target/$BUILD/$b" >&2; exit 1; }
done

cat >"$S/setup.sh" <<'EOS'
#!/bin/bash
# As root in the VM: install, add the users, the drop-in and the flag.
# $1: the mode.
set -euo pipefail
cd ~tester/in
if [ "$1" = greeter ] || [ "$1" = baseline ]; then
    # The greeter is what comes up.
    sed -i 's/^AutomaticLoginEnable=True/AutomaticLoginEnable=False/' /etc/gdm/custom.conf
fi
case $1 in *baseline) exit 0 ;; esac
# Built locally, so unsigned.
rpm -Uvh --nodeps --nosignature --replacepkgs ./skalibs-*.rpm ./execline-*.rpm ./s6-*.rpm
install -d /usr/local/libexec/uinitconverter
install -m 0755 uinitconverterd uinitconverter-runner uinitconverter-timers /usr/local/libexec/uinitconverter/
mkdir -p /etc/uinitconverter
echo "USER_MANAGER=s6" >/etc/uinitconverter/uinitconverter.conf
for u in s6test s6ref; do
    id "$u" >/dev/null 2>&1 || useradd -m "$u"
done
uid=$(id -u s6test)
# greeter: every user's manager.
dir=/etc/systemd/system/user@$uid.service.d
case $1 in greeter | session | apps) dir=/etc/systemd/system/user@.service.d ;; esac
mkdir -p "$dir"
cat >"$dir/90-uinitconverter.conf" <<EOF
[Service]
ExecStart=
ExecStart=/usr/local/libexec/uinitconverter/uinitconverterd user-manager
EOF
# A unit of the user's own, wanted by default.target.
h=/home/s6test/.config/systemd/user
mkdir -p "$h/default.target.wants"
cat >"$h/hello.service" <<EOF
[Service]
ExecStart=/usr/bin/sleep infinity
EOF
ln -sf ../hello.service "$h/default.target.wants/hello.service"
chown -R s6test:s6test /home/s6test/.config
grubby --update-kernel=ALL --args=uinitconverter.user_manager=s6
systemctl daemon-reload
EOS

cat >"$S/check.sh" <<'EOS'
#!/bin/bash
# As root in the VM: start both users' managers and compare.
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
uid=$(id -u s6test) ref=$(id -u s6ref)
cg=/sys/fs/cgroup/user.slice/user-$uid.slice/user@$uid.service
as() {
    local u=$1
    shift
    runuser -u "$u" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$u")" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u "$u")/bus" "$@"
}
grep -q uinitconverter.user_manager=s6 /proc/cmdline && ok "the kernel flag is set" || bad "no flag: $(cat /proc/cmdline)"
t0=$(date +%s%N)
systemctl start "user@$uid.service" && ok "user@$uid.service started (READY=1 arrived)" || bad "user@$uid.service: $(systemctl status "user@$uid.service" 2>&1 | tail -5)"
echo "   start took $(( ($(date +%s%N) - t0) / 1000000 )) ms"
systemctl start "user@$ref.service"
main=$(systemctl show -p MainPID --value "user@$uid.service")
[ "$(cat "/proc/$main/comm" 2>/dev/null)" = uinitconverterd ] && ok "its main process is uinitconverterd" || bad "main PID $main is $(cat "/proc/$main/comm" 2>/dev/null)"
grep -qx "$main" "$cg/init.scope/cgroup.procs" && ok "the manager is in init.scope" || bad "init.scope: $(cat "$cg/init.scope/cgroup.procs" 2>&1)"
sleep 3
for u in default.target sockets.target dbus.socket hello.service; do
    s=$(as s6test systemctl --user is-active "$u" 2>&1)
    [ "$s" = active ] && ok "$u is active" || bad "$u: $s"
done
grep -q . "$cg/app.slice/hello.service/cgroup.procs" 2>/dev/null && ok "hello.service runs in app.slice/hello.service" || bad "no $cg/app.slice/hello.service: $(find "$cg" -maxdepth 2 -type d | sed "s|$cg||" | tr '\n' ' ')"
own=$(as s6test busctl --user status org.freedesktop.systemd1 2>&1 | sed -n 's/^PID=//p')
[ "$own" = "$main" ] && ok "org.freedesktop.systemd1 on the user bus is uinitconverterd" || bad "the bus name's owner: ${own:-none} ($(as s6test busctl --user status org.freedesktop.systemd1 2>&1 | head -3))"
b=$(as s6test systemctl --user show -p ActiveState --value dbus-broker.service 2>&1)
[ "$b" = active ] && ok "dbus-broker.service (the user bus) is active" || bad "dbus-broker.service: $b"
as s6test busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ListNames >/dev/null && ok "a client on the user bus" || bad "ListNames failed"
echo "-- failed units:"
as s6test systemctl --user list-units --failed --no-legend 2>&1 | sed 's/^/   /'
echo "-- units the manager runs:"
as s6test systemctl --user list-units --no-legend --state=active 2>&1 | awk '{print $1}' | tr '\n' ' ' | fold -w 100 | sed 's/^/   /'
echo
# PSS of everything under each user@: the manager, its supervisors, the bus.
pss() {
    local total=0 p v
    for p in $(cat $(find "$1" -name cgroup.procs) 2>/dev/null); do
        v=$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null)
        total=$((total + ${v:-0}))
    done
    echo "$total"
}
procs() { cat $(find "$1" -name cgroup.procs) 2>/dev/null | wc -l; }
refcg=/sys/fs/cgroup/user.slice/user-$ref.slice/user@$ref.service
echo "-- memory (PSS, KiB): s6 $(pss "$cg") in $(procs "$cg") processes; systemd --user $(pss "$refcg") in $(procs "$refcg") processes"
echo "-- by process (s6):"
for p in $(cat $(find "$cg" -name cgroup.procs)); do
    printf '   %6s %-22s %s\n' "$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null)" "$(cat "/proc/$p/comm")" "$(tr '\0' ' ' <"/proc/$p/cmdline" | cut -c1-60)"
done
echo
systemctl reload "user@$uid.service" && ok "reload (notify-reload: RELOADING=1, READY=1)" || bad "reload failed"
systemctl stop "user@$uid.service"
s=$(systemctl show -p ActiveState --value "user@$uid.service")
[ "$s" = inactive ] && ok "user@$uid.service stopped" || bad "user@$uid.service is $s"
left=$(pgrep -u s6test -l || true)
[ -z "$left" ] && ok "no process of s6test is left" || bad "left: $left"
echo "-- the manager's log:"
journalctl -b -u "user@$uid.service" --no-pager -o cat | tail -40 | sed 's/^/   /'
echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/greeter.sh" <<'EOS'
#!/bin/bash
# As root in the VM: GDM's greeter, under whichever user manager runs it.
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
g=$(id -u gdm-greeter)
t=0
until pgrep -u gdm-greeter -x gnome-shell >/dev/null || [ $t -ge 120 ]; do sleep 2; t=$((t + 2)); done
pgrep -u gdm-greeter -x gnome-shell >/dev/null && ok "the greeter's gnome-shell runs (after ${t} s)" || bad "no gnome-shell for gdm-greeter after 120 s"
sleep 20
main=$(systemctl show -p MainPID --value "user@$g.service")
echo "   user@$g.service's main process: $(cat "/proc/$main/comm" 2>/dev/null)"
cg=$(cut -d: -f3 "/proc/$(pgrep -u gdm-greeter -x gnome-shell | head -1)/cgroup" 2>/dev/null)
case $cg in */user@$g.service/session.slice/org.gnome.Shell@gdm.service) ok "gnome-shell is in $cg" ;; *) bad "gnome-shell's cgroup: $cg" ;; esac
as() { runuser -u gdm-greeter -- env XDG_RUNTIME_DIR="/run/user/$g" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$g/bus" "$@"; }
f=$(as systemctl --user list-units --failed --no-legend 2>&1)
[ -z "$f" ] && ok "no failed user units" || bad "failed: $f"
j=$(as systemctl --user list-jobs --no-legend 2>&1)
[ -z "$j" ] && ok "no jobs left" || bad "jobs: $j"
for u in gnome-session@gnome-login.target org.gnome.Shell@gdm.service gnome-session-manager@gnome-login.service pipewire.service; do
    s=$(as systemctl --user is-active "$u" 2>&1)
    [ "$s" = active ] && ok "$u is active" || bad "$u: $s"
done
[ "$(getenforce)" = Enforcing ] && ok "SELinux is enforcing" || bad "SELinux: $(getenforce)"
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
echo "-- the greeter's warnings (journal, priority <= warning):"
journalctl -b "_UID=$g" -p warning --no-pager -o cat | sort | uniq -c | sort -rn | head -25 | cut -c1-160 | sed 's/^/   /'
echo "-- the manager's own lines:"
journalctl -b "_UID=$g" --no-pager -o cat | grep -E "^uinitconverter" | cut -c1-160 | sed 's/^/   /'
echo "-- memory (PSS, KiB) under user@$g.service:"
total=0
# shellcheck disable=SC2046
for p in $(cat $(find "/sys/fs/cgroup/user.slice/user-$g.slice/user@$g.service" -name cgroup.procs)); do
    v=$(awk '/^Pss:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null)
    total=$((total + ${v:-0}))
done
echo "   $total"
echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/session.sh" <<'EOS'
#!/bin/bash
# As root in the VM: tester's GNOME session (GDM autologin), under
# whichever user manager runs it. $1: app (after the app launch) or empty.
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
u=$(id -u tester)
as() { runuser -u tester -- env XDG_RUNTIME_DIR="/run/user/$u" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$u/bus" "$@"; }
if [ "${1:-}" = app ]; then
    t=0
    until p=$(pgrep -n -u tester -f gnome-calculator) || [ $t -ge 30 ]; do sleep 1; t=$((t + 1)); done
    if [ -n "${p:-}" ]; then
        cg=$(cut -d: -f3 "/proc/$p/cgroup")
        case $cg in */user@$u.service/app.slice/app-*gnome-calculator*.scope | */user@$u.service/app.slice/app-*Calculator*.scope) ok "gnome-calculator runs in ${cg##*/}" ;; *) bad "gnome-calculator's cgroup: $cg" ;; esac
        sc=${cg##*/}
        s=$(as systemctl --user is-active "$sc" 2>&1)
        [ "$s" = active ] && ok "$sc is active" || bad "$sc: $s"
    else
        bad "no gnome-calculator after 30 s"
    fi
    echo "-- app scopes:"
    as systemctl --user list-units --no-legend 'app-*' 2>&1 | cut -c1-140 | sed 's/^/   /'
    echo "passed: $pass  failed: $fail"
    [ "$fail" -eq 0 ]
    exit
fi
main=$(systemctl show -p MainPID --value "user@$u.service")
echo "   user@$u.service's main process: $(cat "/proc/$main/comm" 2>/dev/null)"
p=$(pgrep -u tester -x gnome-shell | head -1)
cg=$(cut -d: -f3 "/proc/$p/cgroup" 2>/dev/null)
case $cg in */user@$u.service/session.slice/org.gnome.Shell@user.service) ok "gnome-shell is in $cg" ;; *) bad "gnome-shell's cgroup: $cg" ;; esac
# systemd --user gives its units no ambient capabilities (user@ itself
# has CAP_WAKE_ALARM); with one, bwrap refuses to run (glycin, flatpak).
a=$(sed -n 's/^CapAmb:\t*//p' "/proc/$p/status")
[ "$a" = 0000000000000000 ] && ok "gnome-shell has no ambient capabilities" || bad "gnome-shell's CapAmb: $a"
if journalctl -b "_UID=$u" --no-pager -o cat | grep -q "Failed to load background"; then
    bad "the background did not load (glycin)"
else
    ok "the background loaded"
fi
f=$(as systemctl --user list-units --failed --no-legend 2>&1)
[ -z "$f" ] && ok "no failed user units" || bad "failed: $f"
j=$(as systemctl --user list-jobs --no-legend 2>&1)
[ -z "$j" ] && ok "no jobs left" || bad "jobs: $j"
for x in gnome-session@gnome.target gnome-session-initialized.target graphical-session.target org.gnome.Shell@user.service gnome-session-manager@gnome.service gnome-session-monitor.service org.gnome.SettingsDaemon.Power.service pipewire.service; do
    s=$(as systemctl --user is-active "$x" 2>&1)
    [ "$s" = active ] && ok "$x is active" || bad "$x: $s"
done
echo "   xdg-desktop-autostart.target: $(as systemctl --user is-active xdg-desktop-autostart.target 2>&1)"
echo "   evolution-alarm-notify.service: $(as systemctl --user is-active evolution-alarm-notify.service 2>&1)"
s=$(as systemctl --user is-system-running 2>&1)
[ "$s" = running ] && ok "is-system-running: running" || bad "is-system-running: $s"
e=$(as systemctl --user show-environment 2>&1)
echo "$e" | grep -q '^WAYLAND_DISPLAY=' && ok "the leader's environment reached the manager (WAYLAND_DISPLAY)" || bad "no WAYLAND_DISPLAY in show-environment"
# gnome-session's autostart apps get scopes (app-gnome-NAME-PID.scope).
n=$(as systemctl --user list-units --no-legend 'app-gnome-*.scope' 2>/dev/null | wc -l)
[ "$n" -gt 0 ] && ok "$n autostart app scopes" || bad "no app-gnome-*.scope"
[ "$(getenforce)" = Enforcing ] && ok "SELinux is enforcing" || bad "SELinux: $(getenforce)"
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
echo "-- active user units:"
as systemctl --user list-units --no-legend --state=active 2>&1 | awk '{print $1}' | tr '\n' ' ' | fold -w 110 | sed 's/^/   /'
echo
echo "-- tester's warnings (journal, priority <= warning):"
journalctl -b "_UID=$u" -p warning --no-pager -o cat | sort | uniq -c | sort -rn | head -30 | cut -c1-160 | sed 's/^/   /'
echo "-- the manager's own lines:"
journalctl -b "_UID=$u" --no-pager -o cat | grep -E "^uinitconverter" | cut -c1-160 | tail -40 | sed 's/^/   /'
echo "-- memory (PSS, KiB) under user@$u.service:"
total=0
# shellcheck disable=SC2046
for q in $(cat $(find "/sys/fs/cgroup/user.slice/user-$u.slice/user@$u.service" -name cgroup.procs)); do
    v=$(awk '/^Pss:/ {print $2}' "/proc/$q/smaps_rollup" 2>/dev/null)
    total=$((total + ${v:-0}))
done
echo "   $total"
echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/apps.sh" <<'EOS'
#!/bin/bash
# As root in the VM, in tester's session: slice D (apps and services).
set -uo pipefail
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
u=$(id -u tester)
mgr=/user.slice/user-$u.slice/user@$u.service
as() { runuser -u tester -- env HOME=/home/tester XDG_RUNTIME_DIR="/run/user/$u" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$u/bus" "$@"; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }

echo "== timers"
for x in grub-boot-success.timer systemd-tmpfiles-clean.timer; do
    # One property each: `show -p A,B` prints them in the manager's order.
    s=$(as systemctl --user show -p ActiveState --value "$x" 2>&1)/$(as systemctl --user show -p SubState --value "$x" 2>&1)
    [ "$s" = active/waiting ] && ok "$x is active/waiting" || bad "$x: $s"
done

echo "== sockets"
for x in dbus.socket pipewire.socket pipewire-pulse.socket systemd-importd.socket systemd-machined.socket \
    systemd-ask-password.socket systemd-journalctl.socket systemd-storage-fs.socket; do
    s=$(as systemctl --user is-active "$x" 2>&1)
    [ "$s" = active ] && ok "$x is active" || bad "$x: $s"
done
for v in io.systemd.StorageProvider/fs io.systemd.JournalAccess io.systemd.AskPassword; do
    out=$(as timeout 10 varlinkctl info "unix:/run/user/$u/systemd/$v" 2>&1 | sed -n 's/^ *Product: //p')
    [ -n "$out" ] && ok "Accept=yes $v answers ($out)" || bad "$v: $(as timeout 10 varlinkctl info "unix:/run/user/$u/systemd/$v" 2>&1 | head -2)"
done
echo "   list-sockets: $(as systemctl --user list-sockets --no-legend 2>&1 | wc -l) lines"

echo "== systemd-oomd"
watched() { oomctl 2>/dev/null | sed -n 's/^\tPath: //p'; }
check_oomd() {
    local w x
    until_ok '[ "$(watched | grep -c "^$mgr/")" -ge 3 ]' 15
    w=$(watched)
    # user@UID.service itself (the manager's -.slice): systemd --user
    # reports it only after reconnecting, so it is not required.
    echo "$w" | grep -qxF "$mgr" && echo "   (also $mgr itself)"
    for x in "$mgr/app.slice" "$mgr/background.slice" "$mgr/session.slice"; do
        echo "$w" | grep -qxF "$x" && ok "oomd watches $x$1" || bad "oomd does not watch $x$1: $(echo "$w" | tr '\n' ' ')"
    done
}
check_oomd ""
systemctl restart systemd-oomd
check_oomd " (after oomd restarted)"
oomctl | grep -A2 "Path: $mgr/app.slice$" | sed 's/^/   /'

echo "== podman quadlet"
r=/home/tester/qroot
rm -rf "$r"
mkdir -p "$r/usr/bin" "$r/run" "$r/proc" "$r/sys" "$r/dev" "$r/etc" "$r/tmp"
cp /usr/bin/sleep "$r/usr/bin/"
for l in $(ldd /usr/bin/sleep | grep -o '/[^ ]*'); do mkdir -p "$r$(dirname "$l")"; cp "$l" "$r$l"; done
mkdir -p /home/tester/.config/containers/systemd
printf '[Container]\nRootfs=%%h/qroot\nExec=/usr/bin/sleep infinity\n' >/home/tester/.config/containers/systemd/qhello.container
chown -R tester:tester "$r" /home/tester/.config/containers
# Copied into the home it is user_home_t, which container_t may not map.
chcon -R -t container_file_t "$r"
as systemctl --user daemon-reload
s=$(as systemctl --user show -p LoadState --value qhello.service 2>&1)
[ "$s" = loaded ] && ok "the quadlet generator made qhello.service" || bad "qhello.service: $s"
if as systemctl --user start qhello.service; then ok "qhello.service started (Type=notify via conmon)"; else bad "qhello.service: $(as systemctl --user status qhello.service 2>&1 | tail -6)"; fi
n=$(as podman ps --format '{{.Names}}' 2>&1)
[ "$n" = systemd-qhello ] && ok "podman runs systemd-qhello" || bad "podman ps: $n"
grep -q . "/sys/fs/cgroup$mgr/app.slice/qhello.service/cgroup.procs" 2>/dev/null || [ -n "$(find "/sys/fs/cgroup$mgr/app.slice/qhello.service" -name cgroup.procs -exec cat {} + 2>/dev/null)" ] \
    && ok "the container is in app.slice/qhello.service" || bad "no processes under app.slice/qhello.service"
as systemctl --user stop qhello.service
# conmon exits 143 on the stop: systemd --user marks the unit failed, ours
# inactive (a known difference, work_history part 144). Shown, then cleared.
echo "   qhello.service after the stop: $(as systemctl --user is-active qhello.service 2>&1)"
as systemctl --user reset-failed qhello.service 2>/dev/null
n=$(as podman ps -a --format '{{.Names}}' 2>&1)
[ -z "$n" ] && ok "stopped: the container is gone" || bad "after the stop: $n"

echo "== flatpak"
as flatpak --user remote-add --if-not-exists fedora oci+https://registry.fedoraproject.org
t0=$(date +%s)
if as flatpak --user install -y --noninteractive fedora org.gnome.TextEditor >/tmp/flatpak-install.log 2>&1; then
    ok "org.gnome.TextEditor installed for tester ($(( $(date +%s) - t0 )) s)"
else
    bad "flatpak install: $(tail -3 /tmp/flatpak-install.log)"
fi
d=$(tr '\0' '\n' <"/proc/$(pgrep -u tester -x gnome-shell | head -1)/environ" | sed -n 's/^XDG_DATA_DIRS=//p')
case $d in *"/home/tester/.local/share/flatpak/exports/share"*"/var/lib/flatpak/exports/share"*) ok "gnome-shell's XDG_DATA_DIRS has the flatpak exports" ;; *) bad "gnome-shell's XDG_DATA_DIRS: $d" ;; esac
as env WAYLAND_DISPLAY=wayland-0 setsid flatpak run org.gnome.TextEditor >/tmp/flatpak-run.log 2>&1 &
p=
until_ok 'p=$(pgrep -n -u tester -f "^gnome-text-editor|/app/bin/gnome-text-editor")' 30
if [ -n "$p" ]; then
    cg=$(cut -d: -f3 "/proc/$p/cgroup")
    case $cg in "$mgr"/app.slice/app-flatpak-org.gnome.TextEditor-*.scope) ok "the flatpak app runs in ${cg##*/}" ;; *) bad "the flatpak app's cgroup: $cg" ;; esac
    s=$(as systemctl --user is-active "${cg##*/}" 2>&1)
    [ "$s" = active ] && ok "${cg##*/} is active" || bad "${cg##*/}: $s"
else
    bad "the flatpak app did not start: $(tail -5 /tmp/flatpak-run.log)"
fi

echo "== the whole"
f=$(as systemctl --user list-units --failed --no-legend 2>&1)
[ -z "$f" ] && ok "no failed user units" || bad "failed: $f"
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials: $(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep denied | head -3)"
echo "-- the manager's own lines:"
journalctl -b "_UID=$u" --no-pager -o cat | grep -E "^uinitconverter" | cut -c1-160 | tail -30 | sed 's/^/   /'
echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cp "$TOP/target/$BUILD/uinitconverterd" "$TOP/target/$BUILD/uinitconverter-runner" "$TOP/target/$BUILD/uinitconverter-timers" "$skalibs" "$execline" "$s6" "$S/"
"$VM" start "$D"
"$VM" wait "$D"
"$VM" copy "$D" "$S"/*
"$VM" ssh "$D" sudo bash in/setup.sh "$MODE"
# The flag needs a boot; the overlay is kept.
"$VM" stop "$D"
"$VM" start "$D" --keep
if [ "$MODE" = user ]; then
    "$VM" wait "$D"
    "$VM" ssh "$D" sudo bash in/check.sh
    exit
fi
case $MODE in
    apps | apps-baseline)
        rc=0
        "$VM" wait "$D" || rc=$?
        sleep 20
        "$VM" ssh "$D" sudo bash in/session.sh || rc=$?
        "$VM" ssh "$D" sudo bash in/apps.sh || rc=$?
        "$VM" screenshot "$D" "$D/apps-$MODE.png"
        exit $rc
        ;;
    session | session-baseline)
        rc=0
        "$VM" wait "$D" || rc=$?
        "$VM" check "$D" || rc=$?
        sleep 20
        "$VM" ssh "$D" sudo bash in/session.sh || rc=$?
        # An app from GNOME Shell: the overview is open at login.
        "$VM" key "$D" esc
        sleep 1
        "$VM" key "$D" meta_l
        sleep 2
        "$VM" type "$D" calculator
        sleep 2
        "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/session.sh app || rc=$?
        "$VM" screenshot "$D" "$D/session-$MODE.png"
        exit $rc
        ;;
esac
t=0
until "$VM" ssh "$D" true 2>/dev/null; do
    t=$((t + 5)); [ $t -lt 300 ] || { echo "no SSH after 5 min" >&2; exit 1; }; sleep 5
done
rc=0
"$VM" ssh "$D" sudo bash in/greeter.sh || rc=$?
"$VM" screenshot "$D" "$D/greeter-$MODE.png"
exit $rc
