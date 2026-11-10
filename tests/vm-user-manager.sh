#!/bin/bash
# The user manager, systemd --user, under GNOME in the VM of
# tests/gnome-vm.sh: GDM's greeter, tester's session, apps and services, a
# session's whole life, and SELinux-confined users. BOOT=systemd (the
# default): the stock systemd boot, nothing installed, the reference.
# BOOT=s6: the s6 boot, where the runner starts user@UID.service; the
# same checks, so the two can be compared.
#
#   tests/vm-user-manager.sh DIR [session] # DIR: the provisioned VM (gnome-vm.sh).
#                                        # GDM autologin as tester: gnome-vm.sh
#                                        # wait/check pass, the session's units run,
#                                        # and an app started from GNOME Shell lands
#                                        # in its app-*.scope (DIR/session-session.png)
#   tests/vm-user-manager.sh DIR greeter # autologin off: the greeter must show
#                                        # (DIR/greeter-greeter.png)
#   tests/vm-user-manager.sh DIR apps    # the session as above, then user timers,
#                                        # every user socket (Accept=yes too),
#                                        # systemd-oomd watching the user slices, a
#                                        # podman quadlet container, and a flatpak app
#                                        # (installed for tester from the fedora
#                                        # remote: ~3 GB, network) in its
#                                        # app-flatpak-*.scope
#   tests/vm-user-manager.sh DIR lifecycle # the session, logout, a new login at the
#                                        # greeter, a user switch to alice, the RPM
#                                        # update helper, a lingering user, alice's
#                                        # logout, a reboot
#   tests/vm-user-manager.sh DIR confined # SELinux-confined users: carol (user_u)
#                                        # and dave (staff_u), lingering: user@ comes
#                                        # up in the user's domain, no denials
#
# BOOT=s6 (session, lifecycle, confined): installed as tests/vm-s6-boot.sh
# does it: every RPM (CORE_RPMS=, STANDALONE_RPMS=, SKARNET_RPMS= default
# ~/rpmbuild-skarnet/rpms) and `initconvert init --yes --default-s6
# --selinux=enforcing`.
#
# PRE_REBOOT_SCRIPT=FILE (lifecycle): copied into the VM and run as root
# just before the reboot: first `FILE prepare` (waited for: installing a
# tool), then FILE detached (a sampler that records the shutdown to
# /var/tmp, read after it with `gnome-vm.sh ssh`).
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-user-manager.sh DIR [session|greeter|apps|lifecycle|confined]}
MODE=${2:-session}
case $MODE in session | greeter | apps | lifecycle | confined) ;; *) echo "unknown mode $MODE" >&2; exit 2 ;; esac
BOOT=${BOOT:-systemd}
case $BOOT/$MODE in
    systemd/* | s6/session | s6/lifecycle | s6/confined) ;;
    *) echo "BOOT=$BOOT: not with mode $MODE" >&2; exit 2 ;;
esac
S=$(mktemp -d)
# The VM goes down however this ends (VM_KEEP=yes: it stays up, for
# `gnome-vm.sh ssh DIR`).
if [ "${VM_KEEP:-no}" = yes ]; then
    trap 'rm -rf "$S"' EXIT
else
    trap '"$VM" stop "$D" >/dev/null 2>&1; rm -rf "$S"' EXIT
fi

if [ "$BOOT" = s6 ]; then
    # Every package, the newest build of each (as tests/vm-s6-boot.sh).
    mkdir "$S/rpms"
    newest() { find "$1" -maxdepth 1 -name "$2-[0-9]*.rpm" | sort | tail -1; }
    CORE=${CORE_RPMS:-$D/rpmbuild/core/rpms} STANDALONE=${STANDALONE_RPMS:-$D/rpmbuild/standalone/rpms}
    RPMS=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/rpms}
    pkgs="core:uinitconverter core:uinitconverterd core:uinitconverter-gen core:uinitconverter-runner
        core:uinitconverter-timers standalone:uinitconverter-fdstore
        standalone:uinitconverter-notify standalone:uinitconverter-linkwatch standalone:uinitconverter-journal-relay
        skarnet:skalibs skarnet:execline skarnet:s6 skarnet:s6-rc skarnet:s6-linux-init skarnet:s6-frontend
        skarnet:mdevd skarnet:s6-dns skarnet:s6-networking"
    for p in $pkgs; do
        case ${p%%:*} in core) dir=$CORE ;; standalone) dir=$STANDALONE ;; *) dir=$RPMS ;; esac
        f=$(newest "$dir" "${p#*:}")
        [ -n "$f" ] || { echo "no ${p#*:} RPM in $dir" >&2; exit 1; }
        cp "$f" "$S/rpms/"
    done
fi

cat >"$S/setup.sh" <<'EOS'
#!/bin/bash
# As root in the VM: the users, and the s6 boot's install.
# $1: the mode, $2: the boot (systemd or s6).
set -euo pipefail
cd ~tester/in
if [ "$1" = greeter ]; then
    # The greeter is what comes up.
    sed -i 's/^AutomaticLoginEnable=True/AutomaticLoginEnable=False/' /etc/gdm/custom.conf
fi
# The lifecycle: passwords for the greeter, a second user, a lingering one.
if [ "$1" = lifecycle ]; then
    echo tester:s6Tester1 | chpasswd 2>/dev/null
    for u in alice bob; do id "$u" >/dev/null 2>&1 || useradd -m "$u"; done
    echo alice:s6Alice1 | chpasswd 2>/dev/null
fi
if [ "$2" = s6 ]; then
    # The RPMs and init, as tests/vm-s6-boot.sh; built locally, so unsigned.
    dnf -y -q install --nogpgcheck ./*.rpm || { dnf clean metadata && dnf -y -q install --refresh --nogpgcheck ./*.rpm; }
    # --journal=yes: the checks read the user journal (GNOME's messages, the
    # shutdown timings); JOURNAL=no is checked by tests/vm-s6-boot.sh.
    initconvert init --yes --default-s6 --selinux=enforcing --journal=yes
fi
EOS

cat >"$S/greeter.sh" <<'EOS'
#!/bin/bash
# As root in the VM: GDM's greeter.
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
echo "   user@$g.service's main process: $(basename "$(readlink "/proc/$main/exe")")"
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
# As root in the VM: tester's GNOME session (GDM autologin). $1: app (after the app launch) or empty.
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
echo "   user@$u.service's main process: $(basename "$(readlink "/proc/$main/exe")")"
p=$(pgrep -u tester -x gnome-shell | head -1)
cg=$(cut -d: -f3 "/proc/$p/cgroup" 2>/dev/null)
case $cg in */user@$u.service/session.slice/org.gnome.Shell@user.service) ok "gnome-shell is in $cg" ;; *) bad "gnome-shell's cgroup: $cg" ;; esac
# systemd --user gives its units no ambient capabilities (user@ itself
# has CAP_WAKE_ALARM); with one, bwrap refuses to run (glycin, flatpak).
a=$(sed -n 's/^CapAmb:\t*//p' "/proc/$p/status")
[ "$a" = 0000000000000000 ] && ok "gnome-shell has no ambient capabilities" || bad "gnome-shell's CapAmb: $a"
# grep without -q reads to the end: -q stops at the match, journalctl then
# dies of SIGPIPE and, with pipefail, the match would read as "not found".
if journalctl -b "_UID=$u" --no-pager -o cat | grep -F "Failed to load background" >/dev/null; then
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
# systemd-run reads InvocationID at unit/self from inside the scope; without
# that path it failed ("Unknown object") and Ptyxis fell back to no scope.
out=$(as systemd-run --user --scope --quiet --unit sr-check.scope /bin/sh -c 'cut -d: -f3 /proc/self/cgroup' 2>&1)
[ "$out" = "/user.slice/user-$u.slice/user@$u.service/app.slice/sr-check.scope" ] && ok "systemd-run --user --scope ran in app.slice/sr-check.scope" || bad "systemd-run --scope: $out"
# GetUnitByPID answers the manager's own unit, not user@UID.service.
out=$(as busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager \
    GetUnitByPID u "$(pgrep -u tester -x gnome-shell | head -1)" 2>&1)
case $out in */unit/user_40*) bad "GetUnitByPID of gnome-shell: $out" ;; "o \"/org/freedesktop/systemd1/unit/"*) out=${out##*/}; ok "GetUnitByPID of gnome-shell: ${out%\"}" ;; *) bad "GetUnitByPID of gnome-shell: $out" ;; esac
[ "$(getenforce)" = Enforcing ] && ok "SELinux is enforcing" || bad "SELinux: $(getenforce)"
n=$(ausearch -m avc -ts boot --input-logs </dev/null 2>/dev/null | grep -c denied)
[ "$n" = 0 ] && ok "no AVC denials" || bad "$n AVC denials"
echo "-- active user units:"
as systemctl --user list-units --no-legend --state=active 2>&1 | awk '{print $1}' | tr '\n' ' ' | fold -w 110 | sed 's/^/   /'
echo
echo "-- tester's warnings (journal, priority <= warning):"
journalctl -b "_UID=$u" -p warning --no-pager -o cat | sort | uniq -c | sort -rn | head -30 | cut -c1-160 | sed 's/^/   /'
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
# As root in the VM, in tester's session: apps and services.
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
# conmon exits 143 on the stop: systemd --user marks the unit failed.
# Shown, then cleared.
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
echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

cat >"$S/life.sh" <<'EOS'
#!/bin/bash
# As root in the VM: the lifecycle's steps. life.sh STEP.
set -uo pipefail
step=$1 want=systemd
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
# The manager's executable.
mgr_comm() { basename "$(readlink "/proc/$(systemctl show -p MainPID --value "user@$(id -u "$1").service")/exe")" 2>/dev/null; }
greeter_up() { pgrep -f -u "$(getent passwd | awk -F: '/^gdm-greeter/ {print $1}' | paste -sd,)" '^/usr/bin/gnome-shell' >/dev/null 2>&1; }
session_up() {
    local u=$1 s x
    until_ok '[ -n "$(seat_session '"$u"')" ] && pgrep -u '"$u"' -x gnome-shell >/dev/null' 90 \
        && ok "$u's GNOME session is up" || { bad "$u: no GNOME session after 90 s"; return; }
    s=$(seat_session "$u")
    until_ok '[ "$(loginctl show-session '"$s"' -p Active --value)" = yes ]' 30 && ok "$u's session $s is active" || bad "$u's session $s is not active"
    [ "$(mgr_comm "$u")" = "$want" ] && ok "$u's user manager is $want" || bad "$u's user manager: $(mgr_comm "$u")"
    # The session's transaction goes on after gnome-shell is up: wait for
    # the target, and say how long it took (1-2 s with parallel starts,
    # more is worth a look).
    t=0
    until [ "$(as "$u" systemctl --user is-active gnome-session@gnome.target 2>&1)" = active ] || [ $t -ge 30 ]; do
        sleep 1; t=$((t + 1))
    done
    echo "   $u: gnome-session@gnome.target active ${t} s after gnome-shell was up"
    for x in gnome-session@gnome.target org.gnome.Shell@user.service gnome-session-manager@gnome.service; do
        st=$(as "$u" systemctl --user is-active "$x" 2>&1)
        [ "$st" = active ] && ok "$u: $x is active" || bad "$u: $x: $st"
    done
    x=$(cut -d: -f3 "/proc/$(pgrep -n -u "$u" -x gnome-shell)/cgroup")
    case $x in */user@"$(id -u "$u")".service/session.slice/org.gnome.Shell@user.service) ok "$u's gnome-shell is in its unit" ;; *) bad "$u's gnome-shell: $x" ;; esac
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
}
case $step in
    logout) logout tester
        until_ok greeter_up 60 && ok "the greeter is back" || bad "no greeter after the logout" ;;
    login-tester) session_up tester ;;
    switch)
        # GDM can leave the seat on a session that is still finishing its
        # start when a transient display is asked for then (part 196): wait
        # until tester's shell has registered its display, and a little more.
        p=$(pgrep -n -u tester -x gnome-shell)
        until_ok 'journalctl -b --no-pager -o cat _PID='"$p"' | grep -q "Registering display with GDM"' 30 \
            && ok "tester's gnome-shell registered its display with GDM" || bad "tester's gnome-shell ($p) did not register its display with GDM in 30 s"
        sleep 3
        out=$(as tester gdbus call --system --dest org.gnome.DisplayManager --object-path /org/gnome/DisplayManager/LocalDisplayFactory \
            --method org.gnome.DisplayManager.LocalDisplayFactory.CreateTransientDisplay 2>&1) && ok "a new greeter was asked for ($out)" || bad "CreateTransientDisplay: $out"
        until_ok '[ -n "$(loginctl list-sessions --no-legend | awk '"'"'$6 == "greeter"'"'"')" ]' 60 && ok "the switch greeter runs" || bad "no greeter session"
        # The keys go to whatever seat0 shows: the greeter must be in front.
        until_ok '[ "$(loginctl show-session "$(loginctl show-seat seat0 -p ActiveSession --value)" -p Class --value 2>/dev/null)" = greeter ]' 30 \
            && ok "seat0 shows the switch greeter" || bad "seat0 still shows session $(loginctl show-seat seat0 -p ActiveSession --value), not the switch greeter"
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
        if journalctl -b -1 --no-pager -o cat -t uinitconverter 2>/dev/null | grep -x "shutdown starts" >/dev/null; then
            # The s6 boot (BOOT=s6): from "shutdown starts" to the end of
            # each user@'s PAM session in its runner (pam_unix's close line).
            a=$(journalctl -b -1 -o short-unix --no-pager -t uinitconverter | awk '/shutdown starts/ {print $1; exit}')
            while read -r z u; do
                n=$((n + 1))
                d=$(awk -v a="$a" -v z="$z" 'BEGIN {printf "%.2f", z - a}')
                awk -v d="$d" 'BEGIN {exit !(d < 5)}' && ok "user@ of $u stopped $d s after the shutdown began" || bad "user@ of $u took $d s to stop"
            done < <(journalctl -b -1 -o short-unix --no-pager _COMM=initconvert-run | awk -v a="$a" '$1 >= a && /session closed for user/ {print $1, $NF}')
            [ "$n" -gt 0 ] || bad "no user@ session close after the shutdown began"
            x=$(journalctl -b -1 --no-pager -o cat | grep -E "user@[0-9]+\.service.*(SIGKILL|timed out|failed)" | head -3)
            [ -z "$x" ] && ok "no user@ was killed or failed at the reboot" || bad "$x"
            echo "-- the system manager's slow stops at the reboot:"
            journalctl -b -1 -o short-unix --no-pager _COMM=initconvertd | awk -v a="$a" '$1 >= a && /took|shutdown scopes/ {printf "   +%.2f s ", $1 - a; $1 = ""; $2 = ""; print substr($0, 3)}' | cut -c1-160
            echo "passed: $pass  failed: $fail"
            [ "$fail" -eq 0 ]
            exit
        fi
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

cat >"$S/confined.sh" <<'EOS'
#!/bin/bash
# As root in the VM: SELinux-confined users' managers.
set -uo pipefail
want=systemd
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
until_ok() { local t=0; until eval "$1"; do t=$((t + 1)); [ $t -ge "$2" ] && return 1; sleep 1; done; }
start=$(date +%s)
for u in carol:user_u:user_t dave:staff_u:staff_t; do
    IFS=: read -r n su dom <<<"$u"
    id "$n" >/dev/null 2>&1 || useradd -m "$n"
    semanage login -l | grep -q "^$n " || semanage login -a -s "$su" "$n"
    loginctl enable-linger "$n"
done
for u in carol:user_u:user_t dave:staff_u:staff_t; do
    IFS=: read -r n su dom <<<"$u"
    uid=$(id -u "$n")
    if until_ok '[ "$(systemctl is-active "user@$uid.service")" = active ]' 60; then
        ok "$n ($su): user@$uid is active"
    else
        bad "$n ($su): user@$uid is $(systemctl is-active "user@$uid.service") ($(systemctl show -p Result --value "user@$uid.service"))"
        continue
    fi
    pid=$(systemctl show -p MainPID --value "user@$uid.service")
    exe=$(basename "$(readlink "/proc/$pid/exe")" 2>/dev/null)
    [ "$exe" = "$want" ] && ok "$n's user manager is $want" || bad "$n's user manager is ${exe:-unknown}, not $want"
    ctx=$(tr -d '\0' <"/proc/$pid/attr/current")
    case $ctx in "$su:"*":$dom:"*) ok "$n's manager runs as $ctx" ;; *) bad "$n's manager runs as $ctx, not $dom" ;; esac
done
# Denials from the users' domains (not the usual dontaudit-hidden
# inheritance ones).
other=$(ausearch -m AVC,USER_AVC,SELINUX_ERR -ts "$(date -d @"$start" +%T)" 2>/dev/null | grep -E "scontext=(user|staff)_u:" \
    | grep -vE "\{ (siginh|rlimitinh|noatsecure) \}" || true)
if [ -z "$other" ]; then ok "no denials from the confined users"; else bad "denials:"; printf '%s\n' "$other" | cut -c1-300 | sed 's/^/   /'; fi
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
EOS

"$VM" start "$D"
"$VM" wait "$D"
"$VM" copy "$D" "$S"/*.sh
[ "$BOOT" = s6 ] && "$VM" copy "$D" "$S"/rpms/*.rpm
"$VM" ssh "$D" sudo bash in/setup.sh "$MODE" "$BOOT"
# The s6 boot (and GDM's changed autologin) need a boot; the overlay is kept.
"$VM" stop "$D"
"$VM" start "$D" --keep
case $MODE in
    lifecycle)
        rc=0
        "$VM" wait "$D" || rc=$?
        sleep 20
        "$VM" ssh "$D" sudo bash in/session.sh || rc=$?
        echo "== logout"
        "$VM" ssh "$D" sudo bash in/life.sh logout || rc=$?
        echo "== a new login at the greeter"
        "$VM" key "$D" ret; sleep 2; "$VM" type "$D" s6Tester1; "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/life.sh login-tester || rc=$?
        echo "== a user switch to alice"
        "$VM" ssh "$D" sudo bash in/life.sh switch || rc=$?
        "$VM" key "$D" down; sleep 1; "$VM" key "$D" ret; sleep 2; "$VM" type "$D" s6Alice1; "$VM" key "$D" ret
        "$VM" ssh "$D" sudo bash in/life.sh login-alice || rc=$?
        "$VM" screenshot "$D" "$D/lifecycle-$BOOT.png"
        echo "== a package update of a user unit (the RPM helper)"
        "$VM" ssh "$D" sudo bash in/life.sh update || rc=$?
        echo "== a lingering user"
        "$VM" ssh "$D" sudo bash in/life.sh linger || rc=$?
        echo "== alice logs out"
        "$VM" ssh "$D" sudo bash in/life.sh alice-logout || rc=$?
        echo "== a reboot with tester's session open"
        if [ -n "${PRE_REBOOT_SCRIPT:-}" ]; then
            "$VM" copy "$D" "$PRE_REBOOT_SCRIPT"
            "$VM" ssh "$D" "sudo bash in/${PRE_REBOOT_SCRIPT##*/} prepare" || true
            "$VM" ssh "$D" "sudo setsid -f bash in/${PRE_REBOOT_SCRIPT##*/} </dev/null >/dev/null 2>&1; sleep 1"
        fi
        "$VM" ssh "$D" 'sudo systemctl reboot' || true
        sleep 30
        "$VM" wait "$D" || rc=$?
        "$VM" ssh "$D" sudo bash in/life.sh after-reboot || rc=$?
        exit $rc
        ;;
    confined)
        rc=0
        "$VM" wait "$D" || rc=$?
        "$VM" ssh "$D" sudo bash in/confined.sh || rc=$?
        exit $rc
        ;;
    apps)
        rc=0
        "$VM" wait "$D" || rc=$?
        sleep 20
        "$VM" ssh "$D" sudo bash in/session.sh || rc=$?
        "$VM" ssh "$D" sudo bash in/apps.sh || rc=$?
        "$VM" screenshot "$D" "$D/apps-$BOOT.png"
        exit $rc
        ;;
    session)
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
        "$VM" screenshot "$D" "$D/session-$BOOT.png"
        exit $rc
        ;;
esac
t=0
until "$VM" ssh "$D" true 2>/dev/null; do
    t=$((t + 5)); [ $t -lt 300 ] || { echo "no SSH after 5 min" >&2; exit 1; }; sleep 5
done
rc=0
"$VM" ssh "$D" sudo bash in/greeter.sh || rc=$?
"$VM" screenshot "$D" "$D/greeter-$BOOT.png"
exit $rc
