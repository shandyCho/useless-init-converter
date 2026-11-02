#!/bin/bash
# Step 0 of the user-manager plan (work_history part 136), in the GNOME VM of
# tests/gnome-vm.sh, with the real systemd --user: record every D-Bus message
# to and from org.freedesktop.systemd1 on each user bus (the GDM greeter's
# too) and on the system bus, from boot through an autologin session, an app
# launched from GNOME Shell, and a logout to the greeter; plus memory
# snapshots of the user managers.
#
#   tests/user-inventory.sh VMDIR OUTDIR
#
# The VM must be provisioned (tests/gnome-vm.sh provision VMDIR). It starts
# from a fresh copy and is stopped at the end. OUTDIR gets the captures
# (*.jsonl), mem-*.txt, screenshots and summary.txt
# (tests/inventory/summarize.py).
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
VM=${1:?usage: tests/user-inventory.sh VMDIR OUTDIR}
OUT=${2:?usage: tests/user-inventory.sh VMDIR OUTDIR}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
g() { "$TOP/tests/gnome-vm.sh" "$1" "$VM" "${@:2}"; }
step() { echo "== $*"; }

# Memory of each user manager, and the processes of each user@ cgroup.
cat >"$OUT/mem.sh" <<'EOF'
#!/bin/sh
for p in $(pgrep -f '^/usr/lib/systemd/systemd --user'); do
    u=$(ps -o user= -p "$p")
    awk -v p="$p" -v u="$u" '/^(Rss|Pss|Anonymous):/ {v[$1]=$2} END {printf "manager pid %s user %s Rss %s Pss %s Anon %s kB\n", p, u, v["Rss:"], v["Pss:"], v["Anonymous:"]}' "/proc/$p/smaps_rollup"
done
for d in /sys/fs/cgroup/user.slice/user-*.slice/user@*.service; do
    [ -d "$d" ] || continue
    n=$(find "$d" -name cgroup.procs -exec cat {} + | wc -l)
    echo "cgroup ${d#/sys/fs/cgroup/} processes $n memory.current $(cat "$d/memory.current") units $(find "$d" -mindepth 2 -maxdepth 2 -type d | wc -l)"
done
EOF

step "fresh VM"
g stop >/dev/null 2>&1 || true
g start
g wait
g copy "$TOP/tests/inventory/dbus-capture.py" "$OUT/mem.sh"

step "install the captures"
g ssh 'sudo sh -s' <<'EOF'
set -e
install -D -m 0755 in/dbus-capture.py /usr/local/libexec/inv/dbus-capture.py
install -D -m 0755 in/mem.sh /usr/local/libexec/inv/mem.sh
install -d -m 1777 /var/tmp/inv
cat >/etc/systemd/user/inv-capture.service <<'UNIT'
[Unit]
Description=Step 0: record systemd1 traffic on this user bus
DefaultDependencies=no
Requires=dbus.socket
After=dbus.socket
[Service]
ExecStart=/usr/bin/python3 /usr/local/libexec/inv/dbus-capture.py --session /var/tmp/inv/user-%U.jsonl
[Install]
WantedBy=default.target
UNIT
cat >/etc/systemd/system/inv-sysmon.service <<'UNIT'
[Unit]
Description=Step 0: record systemd1 traffic on the system bus
After=dbus.service
Before=systemd-logind.service gdm.service
[Service]
ExecStart=/usr/bin/python3 /usr/local/libexec/inv/dbus-capture.py --system /var/tmp/inv/system.jsonl
[Install]
WantedBy=multi-user.target
UNIT
systemctl --global enable inv-capture.service
systemctl enable inv-sysmon.service
rm -f /var/tmp/inv/*.jsonl
EOF

step "reboot with the captures in place"
g ssh 'sudo systemctl reboot' || true
sleep 15
g wait
sleep 20
g ssh 'sudo /usr/local/libexec/inv/mem.sh' | tee "$OUT/mem-session.txt"
g screenshot "$OUT/1-session.png" >/dev/null

step "launch Calculator from GNOME Shell (keyboard)"
# The session starts in the overview: leave it, then open it with its search.
g key esc
sleep 2
g key meta_l
sleep 2
g type calculator
sleep 2
g key ret
sleep 8
g screenshot "$OUT/2-app.png" >/dev/null
g ssh 'XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user list-units --no-legend "app-*"; p=$(pgrep -n -f gnome-calculator) && cat /proc/$p/cgroup' | tee "$OUT/app.txt" || true

step "log out to the greeter"
g ssh 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus gnome-session-quit --logout --no-prompt' || true
sleep 30
g screenshot "$OUT/3-greeter.png" >/dev/null
g ssh 'loginctl list-sessions --no-legend; sudo /usr/local/libexec/inv/mem.sh' | tee "$OUT/mem-greeter.txt"

step "collect"
g ssh 'sudo systemctl stop inv-sysmon.service; ls -l /var/tmp/inv >&2; sudo journalctl -b -o short-monotonic --no-pager -t gnome-session -t gnome-session-binary -t gnome-shell -t systemd > in/journal.txt 2>&1; sudo ausearch --input-logs -m AVC -ts boot < /dev/null > in/avc.txt 2>&1 || true; tar -C /var/tmp/inv -cf in/inv.tar . ; tar -C in -cf - inv.tar journal.txt avc.txt' | tar -C "$OUT" -xf -
tar -C "$OUT" -xf "$OUT/inv.tar"
rm -f "$OUT/inv.tar"
g stop >/dev/null
python3 "$TOP/tests/inventory/summarize.py" "$OUT" >"$OUT/summary.txt"
echo "== done: $OUT/summary.txt"
