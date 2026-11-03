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
#   tests/vm-user-manager.sh DIR lifecycle # slice E: the session, logout, a new
#                                        # login at the greeter, a user switch to
#                                        # alice, the RPM update helper, a lingering
#                                        # user, alice's logout, a reboot
#   tests/vm-user-manager.sh DIR lifecycle-baseline # the same on systemd --user
#   tests/vm-user-manager.sh DIR fallback # tester's session broken on purpose for
#                                        # the s6 manager only: systemd --user takes
#                                        # over, the next login works; a crashed
#                                        # manager (a lingering user) too
#
# Copies target/debug's uinitconverterd, -runner and -timers (cargo build
# first) and the skalibs and s6 RPMs from SKARNET_RPMS (default
# ~/rpmbuild-skarnet/RPMS/x86_64, the newest of each) into a fresh overlay.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-user-manager.sh DIR [user|greeter|baseline|session|session-baseline|apps|apps-baseline|lifecycle|lifecycle-baseline|fallback]}
MODE=${2:-user}
case $MODE in user | greeter | baseline | session | session-baseline | apps | apps-baseline | lifecycle | lifecycle-baseline | fallback) ;; *) echo "unknown mode $MODE" >&2; exit 2 ;; esac
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
# Slice E: passwords for the greeter, a second user, a lingering one.
case $1 in
    lifecycle* | fallback)
        echo tester:s6Tester1 | chpasswd 2>/dev/null
        for u in alice bob; do id "$u" >/dev/null 2>&1 || useradd -m "$u"; done
        echo alice:s6Alice1 | chpasswd 2>/dev/null
        ;;
esac
case $1 in *baseline) exit 0 ;; esac
# Built locally, so unsigned.
rpm -Uvh --nodeps --nosignature --replacepkgs ./skalibs-*.rpm ./execline-*.rpm ./s6-*.rpm
install -d /usr/local/libexec/uinitconverter
install -m 0755 uinitconverterd uinitconverter-runner uinitconverter-timers /usr/local/libexec/uinitconverter/
# The fallback's record directory, as the package's tmpfiles.d makes it.
install -Dm0644 uinitconverter-user.conf /etc/tmpfiles.d/uinitconverter-user.conf
systemd-tmpfiles --create /etc/tmpfiles.d/uinitconverter-user.conf
mkdir -p /etc/uinitconverter
echo "USER_MANAGER=s6" >/etc/uinitconverter/uinitconverter.conf
for u in s6test s6ref; do
    id "$u" >/dev/null 2>&1 || useradd -m "$u"
done
uid=$(id -u s6test)
# greeter: every user's manager.
dir=/etc/systemd/system/user@$uid.service.d
case $1 in greeter | session | apps | lifecycle | fallback) dir=/etc/systemd/system/user@.service.d ;; esac
mkdir -p "$dir"
cat >"$dir/90-uinitconverter.conf" <<EOF
[Service]
ExecStart=
ExecStart=/usr/local/libexec/uinitconverter/uinitconverterd user-manager
EOF
if [ "$1" = fallback ]; then
    # Broken for the s6 manager only: a unit path that only it reads (the
    # launcher's test override) starts with a drop-in making tester's
    # gnome-shell fail; systemd --user never sees it.
    mkdir -p "/etc/uinitconverter-break/org.gnome.Shell@user.service.d"
    printf '[Service]\nExecStart=\nExecStart=/bin/false\n' >"/etc/uinitconverter-break/org.gnome.Shell@user.service.d/50-break.conf"
    t=/run/user/1000 h=/home/tester
    mkdir -p /etc/systemd/system/user@1000.service.d
    cat >/etc/systemd/system/user@1000.service.d/95-break.conf <<EOF
[Service]
Environment=UINITCONVERTER_USER_UNIT_PATH=/etc/uinitconverter-break:$t/systemd/transient:$t/systemd/generator.early:$h/.config/systemd/user:/etc/xdg/systemd/user:/etc/systemd/user:$t/systemd/user:/run/systemd/user:$t/systemd/generator:$h/.local/share/systemd/user:/usr/local/share/systemd/user:/usr/share/systemd/user:/usr/local/lib/systemd/user:/usr/lib/systemd/user:$t/systemd/generator.late
EOF
fi
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

cat >"$S/life.sh" <<'EOS'
#!/bin/bash
# As root in the VM: slice E's steps. life.sh STEP MANAGER, MANAGER being
# the comm of the user managers expected (uinitconverterd or systemd).
set -uo pipefail
step=$1 want=${2:-uinitconverterd}
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }
as() {
    local u=$1 id
    shift
    id=$(id -u "$u")
    runuser -u "$u" -- env HOME="/home/$u" XDG_RUNTIME_DIR="/run/user/$id" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$id/bus" "$@"
}
seat_session() { loginctl list-sessions --no-legend | awk -v u="$1" '$3 == u && $4 == "seat0" {print $1}' | head -1; }
mgr_comm() { cat "/proc/$(systemctl show -p MainPID --value "user@$(id -u "$1").service")/comm" 2>/dev/null; }
greeter_up() { pgrep -f -u "$(getent passwd | awk -F: '/^gdm-greeter/ {print $1}' | paste -sd,)" '^/usr/bin/gnome-shell' >/dev/null 2>&1; }
no_fallback() {
    [ "$want" = uinitconverterd ] || return 0
    [ ! -e "/run/uinitconverter-user/$(id -u "$1")" ] && ok "no fallback for $1" || bad "$1 fell back: $(cat "/run/uinitconverter-user/$(id -u "$1")")"
}
session_up() {
    local u=$1 s x
    until_ok '[ -n "$(seat_session '"$u"')" ] && pgrep -u '"$u"' -x gnome-shell >/dev/null' 90 \
        && ok "$u's GNOME session is up" || { bad "$u: no GNOME session after 90 s"; return; }
    s=$(seat_session "$u")
    until_ok '[ "$(loginctl show-session '"$s"' -p Active --value)" = yes ]' 30 && ok "$u's session $s is active" || bad "$u's session $s is not active"
    [ "$(mgr_comm "$u")" = "$want" ] && ok "$u's user manager is $want" || bad "$u's user manager: $(mgr_comm "$u")"
    sleep 5
    for x in gnome-session@gnome.target org.gnome.Shell@user.service gnome-session-manager@gnome.service; do
        st=$(as "$u" systemctl --user is-active "$x" 2>&1)
        [ "$st" = active ] && ok "$u: $x is active" || bad "$u: $x: $st"
    done
    x=$(cut -d: -f3 "/proc/$(pgrep -n -u "$u" -x gnome-shell)/cgroup")
    case $x in */user@"$(id -u "$u")".service/session.slice/org.gnome.Shell@user.service) ok "$u's gnome-shell is in its unit" ;; *) bad "$u's gnome-shell: $x" ;; esac
    no_fallback "$u"
}
logout() {
    local u=$1 s old x f
    s=$(seat_session "$u")
    old=$(as "$u" systemctl --user show -p MainPID --value dbus-broker.service)
    as "$u" gnome-session-quit --logout --no-prompt &
    until_ok '! loginctl show-session '"$s"' >/dev/null 2>&1 || [ "$(loginctl show-session '"$s"' -p State --value)" = closing ]' 60 \
        && ok "$u's session $s ended" || bad "$u's session $s still: $(loginctl show-session "$s" -p State --value)"
    sleep 5
    x=$(systemctl is-active "user@$(id -u "$u").service")
    echo "   user@$(id -u "$u").service: $x (an ssh login or logind's stop delay keeps it)"
    if [ "$x" = active ]; then
        for x in gnome-session@gnome.target org.gnome.Shell@user.service gnome-session-manager@gnome.service gnome-session-monitor.service; do
            st=$(as "$u" systemctl --user is-active "$x" 2>&1)
            [ "$st" != active ] && ok "$u: $x is $st" || bad "$u: $x is still active"
        done
        echo "   dbus-broker: $old -> $(as "$u" systemctl --user show -p MainPID --value dbus-broker.service) (gnome-session-ctl --restart-dbus)"
        j=$(as "$u" systemctl --user list-jobs --no-legend 2>&1)
        [ -z "$j" ] && ok "$u: no jobs left" || bad "$u: jobs: $j"
        f=$(as "$u" systemctl --user list-units --failed --no-legend 2>&1)
        echo "   failed after logout: ${f:-none}"
        case $f in *gnome-session* | *org.gnome.Shell*) bad "$u: session units failed: $f" ;; *) ok "$u: no session unit failed" ;; esac
    fi
    no_fallback "$u"
}
case $step in
    logout) logout tester
        until_ok greeter_up 60 && ok "the greeter is back" || bad "no greeter after the logout" ;;
    login-tester) session_up tester ;;
    switch)
        out=$(as tester gdbus call --system --dest org.gnome.DisplayManager --object-path /org/gnome/DisplayManager/LocalDisplayFactory \
            --method org.gnome.DisplayManager.LocalDisplayFactory.CreateTransientDisplay 2>&1) && ok "a new greeter was asked for ($out)" || bad "CreateTransientDisplay: $out"
        until_ok '[ -n "$(loginctl list-sessions --no-legend | awk '"'"'$6 == "greeter"'"'"')" ]' 60 && ok "the switch greeter runs" || bad "no greeter session"
        sleep 10 ;;
    login-alice)
        session_up alice
        t=$(seat_session tester)
        [ -n "$t" ] && pgrep -u tester -x gnome-shell >/dev/null && ok "tester's session $t goes on beside alice's" || bad "tester's session is gone"
        [ "$(loginctl show-session "$t" -p Active --value)" = no ] && ok "tester's session is in the background" || bad "tester's session is active" ;;
    update)
        p1=$(as tester systemctl --user show -p MainPID --value pipewire.service)
        p2=$(as alice systemctl --user show -p MainPID --value pipewire.service)
        m1=$(systemctl show -p MainPID --value user@1000.service)
        /usr/lib/systemd/systemd-update-helper mark-restart-user-units pipewire.service
        until_ok '[ "$(systemctl --user -M tester@ show -p Markers --value pipewire.service 2>/dev/null)" = needs-restart ]' 10 \
            && ok "systemctl --user -M tester@ set-property Markers= reached tester's manager" || bad "Markers: $(systemctl --user -M tester@ show -p Markers --value pipewire.service 2>&1)"
        /usr/lib/systemd/systemd-update-helper user-reload-restart
        sleep 8
        n1=$(as tester systemctl --user show -p MainPID --value pipewire.service)
        n2=$(as alice systemctl --user show -p MainPID --value pipewire.service)
        [ "$n1" != "$p1" ] && [ "$n1" != 0 ] && ok "tester's pipewire restarted ($p1 -> $n1)" || bad "tester's pipewire: $p1 -> $n1"
        [ "$n2" != "$p2" ] && [ "$n2" != 0 ] && ok "alice's pipewire restarted ($p2 -> $n2)" || bad "alice's pipewire: $p2 -> $n2"
        [ -z "$(as tester systemctl --user show -p Markers --value pipewire.service)" ] && ok "the marker is cleared" || bad "Markers left"
        [ "$(systemctl show -p MainPID --value user@1000.service)" = "$m1" ] && [ "$(systemctl is-active user@1000.service)" = active ] \
            && ok "user@1000 reloaded in place (main PID $m1)" || bad "user@1000: $(systemctl is-active user@1000.service), main $(systemctl show -p MainPID --value user@1000.service) (was $m1)" ;;
    linger)
        b=$(id -u bob)
        loginctl enable-linger bob
        until_ok '[ "$(systemctl is-active user@'"$b"'.service)" = active ]' 30 && ok "lingering: user@$b started without a session" || bad "user@$b: $(systemctl is-active "user@$b.service")"
        [ "$(mgr_comm bob)" = "$want" ] && ok "bob's manager is $want" || bad "bob's manager: $(mgr_comm bob)"
        st=$(systemctl --user -M bob@ is-active default.target 2>&1)
        [ "$st" = active ] && ok "systemctl --user -M bob@: default.target active" || bad "-M bob@: $st"
        loginctl disable-linger bob
        until_ok '[ "$(systemctl is-active user@'"$b"'.service)" = inactive ]' 60 && ok "disable-linger stopped user@$b" || bad "user@$b: $(systemctl is-active "user@$b.service")"
        [ -z "$(pgrep -u bob)" ] && ok "no process of bob is left" || bad "bob's processes: $(pgrep -a -u bob)" ;;
    alice-logout)
        logout alice
        t=$(seat_session tester)
        loginctl activate "$t"
        until_ok '[ "$(loginctl show-session '"$t"' -p Active --value)" = yes ]' 30 && ok "back to tester's session" || bad "tester's session is not active" ;;
    after-reboot)
        n=0
        while read -r u; do
            a=$(journalctl -b -1 -o short-unix --no-pager "_PID=1" | awk -v u="$u" '$0 ~ "Stopping " u "\\.service" {print $1; exit}')
            z=$(journalctl -b -1 -o short-unix --no-pager "_PID=1" | awk -v u="$u" '$0 ~ "Stopped " u "\\.service" {print $1; exit}')
            [ -n "$a" ] && [ -n "$z" ] || continue
            n=$((n + 1))
            d=$(awk -v a="$a" -v z="$z" 'BEGIN {printf "%.2f", z - a}')
            awk -v d="$d" 'BEGIN {exit !(d < 5)}' && ok "$u stopped in $d s at the reboot" || bad "$u took $d s to stop"
        done < <(journalctl -b -1 --no-pager -o cat "_PID=1" | sed -n 's/^Stopping \(user@[0-9]*\)\.service.*/\1/p' | sort -u)
        [ "$n" -gt 0 ] || bad "no user@ stop in the last boot's journal"
        x=$(journalctl -b -1 --no-pager -o cat "_PID=1" | grep -E "user@[0-9]+\.service: (State 'stop-sigterm' timed out|Killing process|Failed with result)" | head -3)
        [ -z "$x" ] && ok "no user@ was killed or failed at the reboot" || bad "$x" ;;
esac
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/fallback.sh" <<'EOS'
#!/bin/bash
# As root in the VM: the fallback, after tester's broken autologin.
set -uo pipefail
step=$1
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }
mgr_comm() { cat "/proc/$(systemctl show -p MainPID --value "user@$1.service")/comm" 2>/dev/null; }
case $step in
    broken)
        until_ok '[ -e /run/uinitconverter-user/1000 ]' 120 && ok "tester's s6 manager fell back: $(cat /run/uinitconverter-user/1000)" || bad "no fallback record after 120 s"
        # gnome-shell failing at its start fails the leader's start of the
        # session target; failing later, it fails into the shutdown target.
        case $(cat /run/uinitconverter-user/1000 2>/dev/null) in
            *"org.gnome.Shell@user.service failed (OnFailure=gnome-session-shutdown.target)"* | *"the start of gnome-session@gnome.target ended"*) ok "for GNOME's failure path" ;;
            *) bad "reason: $(cat /run/uinitconverter-user/1000 2>&1)" ;;
        esac
        [ "$(stat -c %U /run/uinitconverter-user/1000)" = tester ] && ok "the record is tester's own file" || bad "owner: $(stat -c %U /run/uinitconverter-user/1000)"
        until_ok '[ "$(systemctl is-active user@1000.service)" != active ] || [ "$(mgr_comm 1000)" = systemd ]' 30
        echo "   user@1000.service: $(systemctl is-active user@1000.service), main $(mgr_comm 1000)"
        [ -z "$(pgrep -u tester -x s6-svscan)$(pgrep -u tester -x s6-supervise)$(pgrep -u tester -f '^uinitconverter-runner|/uinitconverter-runner ')" ] \
            && ok "no s6 process of tester is left" || bad "left: $(pgrep -a -u tester -x s6-svscan; pgrep -a -u tester -x s6-supervise)"
        if [ "$(systemctl is-active user@1000.service)" = active ]; then
            cg=/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service
            [ ! -e "$cg/app.slice/uinitconverter" ] && ok "user@1000 is systemd --user's now" || bad "cgroup: $(ls "$cg")"
        fi
        journalctl -b _UID=1000 --no-pager -o cat | grep -E "^uinitconverterd.*(handing over|fallback|fell back)" | sed 's/^/   /'
        until_ok 'pgrep -f -u "$(getent passwd | awk -F: '"'"'/^gdm-greeter/ {print $1}'"'"' | paste -sd,)" "^/usr/bin/gnome-shell" >/dev/null' 90 \
            && ok "the greeter is back" || bad "no greeter" ;;
    relogin)
        until_ok 'pgrep -u tester -x gnome-shell >/dev/null' 90 && ok "tester's GNOME session is up after the fallback" || bad "no gnome-shell for tester"
        [ "$(mgr_comm 1000)" = systemd ] && ok "under systemd --user" || bad "user@1000's main: $(mgr_comm 1000)"
        sleep 5
        x=$(cut -d: -f3 "/proc/$(pgrep -n -u tester -x gnome-shell)/cgroup")
        case $x in */user@1000.service/session.slice/org.gnome.Shell@user.service) ok "gnome-shell in its unit" ;; *) bad "gnome-shell: $x" ;; esac
        st=$(runuser -u tester -- env XDG_RUNTIME_DIR=/run/user/1000 systemctl --user is-active gnome-session@gnome.target 2>&1)
        [ "$st" = active ] && ok "gnome-session@gnome.target is active" || bad "gnome-session@gnome.target: $st" ;;
    crash)
        b=$(id -u bob)
        loginctl enable-linger bob
        until_ok '[ "$(systemctl is-active user@'"$b"'.service)" = active ]' 30
        [ "$(mgr_comm "$b")" = uinitconverterd ] && ok "bob's manager is the s6 one" || bad "bob's manager: $(mgr_comm "$b")"
        [ -e "/run/uinitconverter-user/$b.running" ] && ok "its running stamp is there" || bad "no running stamp"
        # SIGKILL (the OOM killer): a Rust program ignores a SIGSEGV sent
        # with kill (its handler only acts on a real fault).
        kill -KILL "$(systemctl show -p MainPID --value "user@$b.service")"
        until_ok '[ "$(systemctl is-active user@'"$b"'.service)" != active ]' 30
        echo "   after the crash: user@$b.service $(systemctl is-active "user@$b.service")"
        systemctl reset-failed "user@$b.service" 2>/dev/null
        systemctl start "user@$b.service"
        until_ok '[ "$(mgr_comm '"$b"')" = systemd ]' 30 && ok "the next start of user@$b runs systemd --user" || bad "bob's manager: $(mgr_comm "$b")"
        case $(cat "/run/uinitconverter-user/$b" 2>/dev/null) in *"ended without stopping"*) ok "recorded as a crash" ;; *) bad "record: $(cat "/run/uinitconverter-user/$b" 2>&1)" ;; esac
        st=$(systemctl --user -M bob@ is-active default.target 2>&1)
        [ "$st" = active ] && ok "and it works (default.target active)" || bad "-M bob@: $st"
        loginctl disable-linger bob ;;
esac
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials: $(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep denied | head -2)"
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cp "$TOP/target/$BUILD/uinitconverterd" "$TOP/target/$BUILD/uinitconverter-runner" "$TOP/target/$BUILD/uinitconverter-timers" "$skalibs" "$execline" "$s6" "$TOP/dist/tmpfiles.d/uinitconverter-user.conf" "$S/"
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
    lifecycle | lifecycle-baseline)
        want=uinitconverterd
        [ "$MODE" = lifecycle-baseline ] && want=systemd
        rc=0
        "$VM" wait "$D" || rc=$?
        sleep 20
        "$VM" ssh "$D" sudo bash in/session.sh || rc=$?
        echo "== logout"
        "$VM" ssh "$D" sudo bash in/life.sh logout $want || rc=$?
        echo "== a new login at the greeter"
        "$VM" key "$D" ret; sleep 2; "$VM" type "$D" s6Tester1; "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/life.sh login-tester $want || rc=$?
        echo "== a user switch to alice"
        "$VM" ssh "$D" sudo bash in/life.sh switch $want || rc=$?
        "$VM" key "$D" down; sleep 1; "$VM" key "$D" ret; sleep 2; "$VM" type "$D" s6Alice1; "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/life.sh login-alice $want || rc=$?
        "$VM" screenshot "$D" "$D/lifecycle-$MODE.png"
        echo "== a package update of a user unit (the RPM helper)"
        "$VM" ssh "$D" sudo bash in/life.sh update $want || rc=$?
        echo "== a lingering user"
        "$VM" ssh "$D" sudo bash in/life.sh linger $want || rc=$?
        echo "== alice logs out"
        "$VM" ssh "$D" sudo bash in/life.sh alice-logout $want || rc=$?
        echo "== a reboot with tester's session open"
        "$VM" ssh "$D" 'sudo systemctl reboot' || true
        sleep 30
        "$VM" wait "$D" || rc=$?
        "$VM" ssh "$D" sudo bash in/life.sh after-reboot $want || rc=$?
        exit $rc
        ;;
    fallback)
        rc=0
        t=0
        until "$VM" ssh "$D" true 2>/dev/null; do
            t=$((t + 5)); [ $t -lt 300 ] || { echo "no SSH after 5 min" >&2; exit 1; }; sleep 5
        done
        echo "== tester's autologin, broken for the s6 manager"
        "$VM" ssh "$D" sudo bash in/fallback.sh broken || rc=$?
        "$VM" screenshot "$D" "$D/fallback-greeter.png"
        echo "== tester logs in again"
        # The greeter settles after the failed autologin first.
        sleep 20
        "$VM" key "$D" ret; sleep 3; "$VM" type "$D" s6Tester1; "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/fallback.sh relogin || rc=$?
        "$VM" screenshot "$D" "$D/fallback-session.png"
        echo "== a crashed manager (lingering bob)"
        "$VM" ssh "$D" sudo bash in/fallback.sh crash || rc=$?
        exit $rc
        ;;
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
