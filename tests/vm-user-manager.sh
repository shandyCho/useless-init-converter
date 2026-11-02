#!/bin/bash
# The s6 user manager under a real user@UID.service (slice A, work_history
# part 141), in the GNOME VM of tests/gnome-vm.sh. The VM boots systemd; a
# drop-in makes one test user's user@ run `uinitconverterd user-manager`,
# and uinitconverter.user_manager=s6 on the kernel command line lets it
# (the launcher hands over to systemd --user on a systemd boot otherwise).
# GDM, the greeter and tester's GNOME session keep systemd --user. A second
# user with systemd --user is the memory comparison.
#
#   tests/vm-user-manager.sh DIR     # DIR: the provisioned VM (gnome-vm.sh)
#
# Copies target/debug's uinitconverterd, -runner and -timers (cargo build
# first) and the skalibs and s6 RPMs from SKARNET_RPMS (default
# ~/rpmbuild-skarnet/RPMS/x86_64, the newest of each) into a fresh overlay.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=$TOP/tests/gnome-vm.sh
D=${1:?usage: tests/vm-user-manager.sh DIR}
RPMS=${SKARNET_RPMS:-$HOME/rpmbuild-skarnet/RPMS/x86_64}
S=$(mktemp -d)
# The VM goes down however this ends.
trap '"$VM" stop "$D" >/dev/null 2>&1; rm -rf "$S"' EXIT

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
set -euo pipefail
cd ~tester/in
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
mkdir -p "/etc/systemd/system/user@$uid.service.d"
cat >"/etc/systemd/system/user@$uid.service.d/90-uinitconverter.conf" <<EOF
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

cp "$TOP/target/$BUILD/uinitconverterd" "$TOP/target/$BUILD/uinitconverter-runner" "$TOP/target/$BUILD/uinitconverter-timers" "$skalibs" "$execline" "$s6" "$S/"
"$VM" start "$D"
"$VM" wait "$D"
"$VM" copy "$D" "$S"/*
"$VM" ssh "$D" sudo bash in/setup.sh
# The flag needs a boot; the overlay is kept.
"$VM" stop "$D"
"$VM" start "$D" --keep
"$VM" wait "$D"
"$VM" ssh "$D" sudo bash in/check.sh
