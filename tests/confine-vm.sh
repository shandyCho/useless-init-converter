#!/bin/bash
# Run the s6 services started through `initconvert-runner confine` (rsyslog,
# earlyoom, udevd) as real root, in QEMU, unprivileged: the running kernel, an
# initramfs with this host's programs and unit files, dist/s6's run scripts
# under s6-supervise. No SELinux policy is loaded in there; that takes a real
# boot. Also runs dist/s6-linux-init/init-selinux, for the hard RLIMIT_NOFILE and soft RLIMIT_CORE,
# with the policy load and s6-linux-init replaced by stand-ins.
#
#   tests/confine-vm.sh WORKDIR [KVER]
#
# Needs a readable /boot/vmlinuz-KVER (default: the running kernel), QEMU,
# /dev/kvm (else very slow), cpio, `cargo build` done, and rsyslog, earlyoom and
# s6 installed.
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
W=${1:?usage: tests/confine-vm.sh WORKDIR [KVER]}
KVER=${2:-$(uname -r)}
KERNEL=/boot/vmlinuz-$KVER
[ -r "$KERNEL" ] || { echo "$KERNEL not readable" >&2; exit 1; }
[ -x "$TOP/target/debug/initconvert-runner" ] || { echo "run cargo build first" >&2; exit 1; }
rm -rf "$W"
R=$W/root
mkdir -p "$R"/usr/bin "$R"/usr/sbin "$R"/usr/lib64 "$R"/usr/lib/systemd/system
ln -s usr/bin "$R/bin"
ln -s usr/sbin "$R/sbin"
ln -s usr/lib64 "$R/lib64"
ln -s usr/lib "$R/lib"

# A program or library and the libraries it needs, under its own path.
add() {
    local f l
    for f in "$@"; do
        [ -e "$R$f" ] && continue
        mkdir -p "$R$(dirname "$f")"
        cp -L "$f" "$R$f"
        for l in $(ldd "$f" 2>/dev/null | sed -n 's/.*=> \(\/[^ ]*\).*/\1/p; s/^[[:space:]]*\(\/[^ ]*\) .*/\1/p'); do
            add "$l"
        done
    done
}
for p in bash sh cat sed grep awk cut seq readlink mkdir ls stat kill sleep mount umount mv test rm tr id true chmod cp \
    tail logger mountpoint rsyslogd earlyoom udevadm s6-svscan s6-supervise s6-svstat s6-svc \
    s6-notifyoncheck execlineb; do
    add "$(type -P "$p")"
done
add /usr/lib/systemd/systemd-udevd
# Opened at run time, so ldd doesn't list them.
add /usr/lib64/libseccomp.so.2 /usr/lib64/libkmod.so.2 /usr/lib64/libzstd.so.1
# s6-notifyoncheck's helper, wherever s6 keeps it.
add "$(rpm -ql s6 | grep -E '/s6-ftrigrd$' | head -n 1)"
for m in /usr/lib64/rsyslog/*.so; do add "$m"; done
mkdir -p "$R/usr/libexec/uinitconverter"
cp "$TOP/target/debug/initconvert-runner" "$R/usr/libexec/uinitconverter/"
add "$TOP/target/debug/initconvert-runner"
rm -rf "$R${TOP:?}"
cp /usr/lib/systemd/system/{rsyslog,earlyoom,systemd-udevd}.service "$R/usr/lib/systemd/system/"
mkdir -p "$R/usr/lib/udev"
cp -r /usr/lib/udev/rules.d "$R/usr/lib/udev/"
mkdir -p "$R"/etc/uinitconverter "$R"/etc/default "$R"/etc/udev/rules.d "$R"/service \
    "$R"/var/log "$R"/var/lib/rsyslog "$R"/var/tmp "$R"/proc "$R"/sys "$R"/dev "$R"/run \
    "$R"/tmp "$R"/home "$R"/root "$R"/mnt/late
echo 'root:x:0:0:root:/root:/bin/bash
earlyoom:x:990:990::/:/usr/sbin/nologin
puser:x:991:991::/:/usr/sbin/nologin' >"$R/etc/passwd"
echo 'root:x:0:
earlyoom:x:990:
puser:x:991:
pextra:x:992:puser' >"$R/etc/group"
echo 'passwd: files
group: files' >"$R/etc/nsswitch.conf"
echo 'JOURNAL=no' >"$R/etc/uinitconverter/uinitconverter.conf"
cp "$TOP/dist/rsyslog.conf" "$R/etc/uinitconverter/"
echo "EARLYOOM_ARGS=\"-r 0 -m 4 -M 409600 --avoid '^(no such|program)\$'\"" >"$R/etc/default/earlyoom"
# shellcheck disable=SC2016  # for udevd
echo 'KERNEL=="null", ACTION=="add", ENV{VM_TEST}="yes", RUN+="/bin/sh -c '\''id -u >/run/vm-rule-ran'\''"' >"$R/etc/udev/rules.d/99-vm.rules"
cat /etc/machine-id >"$R/etc/machine-id"
for s in syslog earlyoom udevd; do
    mkdir "$R/service/$s"
    cp "$TOP/dist/s6/$s/run" "$R/service/$s/"
done
cp "$TOP/dist/s6/udevd/notification-fd" "$R/service/udevd/"
# PrivateUsers=: what the process sees, one "key value" per line.
cat >"$R/usr/bin/pu-probe" <<'PROBE'
#!/bin/sh
map() { awk '{ printf "%s%s %s %s", s, $1, $2, $3; s = "," } END { print "" }' "/proc/self/$1"; }
{
    echo "uid $(id -u)"
    echo "gid $(id -g)"
    echo "groups $(id -G)"
    echo "uid_map $(map uid_map)"
    echo "gid_map $(map gid_map)"
    echo "setgroups $(cat /proc/self/setgroups)"
    echo "ns $(readlink /proc/self/ns/user)"
    echo "capbnd $(sed -n 's/^CapBnd:[[:space:]]*//p' /proc/self/status)"
    echo "capeff $(sed -n 's/^CapEff:[[:space:]]*//p' /proc/self/status)"
    if [ -d /pu-root/usr ]; then echo "chroot no"; else echo "chroot yes"; fi
    mkdir -p /tmp/pu-mnt 2>/dev/null
    if mount -t tmpfs none /tmp/pu-mnt 2>/dev/null; then echo "mount allowed"; else echo "mount refused"; fi
    # Root in its namespace owns /, so only a read-only mount stops this.
    if touch /pu-rootfs 2>/dev/null; then echo "rootfs rw"; rm -f /pu-rootfs; else echo "rootfs ro"; fi
} >"$1.tmp"
mv "$1.tmp" "$1"
PROBE
chmod +x "$R/usr/bin/pu-probe"
pu_unit() { # name, then the [Service] lines
    local name=$1
    shift
    {
        echo '[Service]'
        echo "ExecStart=/usr/bin/pu-probe /run/pu/$name"
        printf '%s\n' "$@"
    } >"$R/usr/lib/systemd/system/$name.service"
}
pu_unit pu-none User=puser
pu_unit pu-user User=puser PrivateUsers=yes CapabilityBoundingSet=
pu_unit pu-root PrivateUsers=yes CapabilityBoundingSet=
pu_unit pu-caps PrivateUsers=yes
pu_unit pu-identity PrivateUsers=identity
pu_unit pu-full PrivateUsers=full
pu_unit pu-no PrivateUsers=no
pu_unit pu-chroot PrivateUsers=yes RootDirectory=/pu-root
pu_unit pu-strict PrivateUsers=yes ProtectSystem=strict
# init-selinux, with stand-ins for what it loads and execs.
cp "$TOP/dist/s6-linux-init/init-selinux" "$R/usr/sbin/"
printf '#!/bin/sh\nexit 0\n' >"$R/usr/sbin/load_policy"
mkdir -p "$R/etc/s6-linux-init/current/bin"
cat >"$R/etc/s6-linux-init/current/bin/init" <<'STANDIN'
#!/bin/bash
echo "$* $(ulimit -Sn) $(ulimit -Hn) $(ulimit -Sn 65535 2>/dev/null && ulimit -Sn) $(ulimit -Sc)"
STANDIN
chmod +x "$R/usr/sbin/init-selinux" "$R/usr/sbin/load_policy" "$R/etc/s6-linux-init/current/bin/init"

cat >"$R/init" <<'EOF'
#!/bin/bash
/usr/bin/mount -t devtmpfs dev /dev
exec </dev/console >/dev/console 2>&1
export PATH=/usr/bin:/usr/sbin
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t tmpfs -o mode=755 run /run
mount -t cgroup2 cgroup /sys/fs/cgroup
mount --make-rshared /

ok() { echo "PASS $1"; }
bad() { echo "FAIL $1"; }
is() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', expected '$3'"; fi; }
# Up to 30 s for a condition.
wait_for() {
    local i
    for i in $(seq 1 150); do
        eval "$1" && return 0
        sleep 0.2
    done
    return 1
}
pid_of() {
    local p c last=
    for p in /proc/[0-9]*; do
        read -r c 2>/dev/null <"$p/comm" || continue
        [ "$c" = "$1" ] && last=${p#/proc/}
    done
    echo "$last"
}
# The supervised process has become a new udevd. Asks s6 for the PID: looked up
# by name right after a kill, it sometimes came back empty.
udevd_changed() {
    local p
    p=$(s6-svstat -p /service/udevd)
    [ "$p" != "$u" ] && [ "$(cat "/proc/$p/comm" 2>/dev/null)" = systemd-udevd ]
}
status() { sed -n "s/^$2:[[:space:]]*//p" "/proc/$1/status"; }
top_mount() { awk -v p="$2" '$5 == p { l = $6 } END { print l }' "/proc/$1/mountinfo" | cut -d, -f1; }

echo '--- init-selinux'
is "the kernel's limit, before init-selinux" "$(ulimit -Sn):$(ulimit -Hn)" 1024:4096
# Arguments passed on, soft limit kept, hard limit raised, and a program can then
# raise its soft limit as cups does (past the old hard limit even root can't).
is "the kernel's core limit" "$(ulimit -Sc)" 0
is "init-selinux: raises only the hard limit, and core dumps" "$(/usr/sbin/init-selinux a b 2>&1)" "a b 1024 524288 65535 unlimited"
# Can't raise it: the boot goes on, with the limit it had.
nr=$(cat /proc/sys/fs/nr_open)
echo 100000 >/proc/sys/fs/nr_open
is "init-selinux: boots anyway if it can't" "$(/usr/sbin/init-selinux a 2>/dev/null)" "a 1024 4096  unlimited"
/usr/sbin/init-selinux a 2>&1 | grep -q "can't raise" && ok "init-selinux: says so" || bad "init-selinux: no message"
echo "$nr" >/proc/sys/fs/nr_open

s6-svscan /service &
wait_for '[ -n "$(pid_of systemd-udevd)" ] && [ -S /run/udev/control ]' || bad "udevd didn't start"
wait_for 's6-svstat /service/udevd | grep -q ready' || bad "udevd not ready"
u=$(pid_of systemd-udevd)

if grep -q uinitconverter.udev=plain /proc/cmdline; then
    is "udev=plain: no seccomp" "$(status "$u" Seccomp)" 0
    is "udev=plain: root cgroup" "$(cat "/proc/$u/cgroup")" "0::/"
    is "udev=plain: OOM score" "$(cat "/proc/$u/oom_score_adj")" -1000
    echo VM-DONE
    # The kernel's "Power Off" line can land inside the marker otherwise.
    sleep 1
    echo o >/proc/sysrq-trigger
    sleep 60
fi

echo '--- udevd'
is "udevd: cgroup" "$(cat "/proc/$u/cgroup")" "0::/system.slice/systemd-udevd.service"
is "udevd: seccomp" "$(status "$u" Seccomp)" 2
is "udevd: OOM score" "$(cat "/proc/$u/oom_score_adj")" -1000
b=$((16#$(status "$u" CapBnd)))
is "udevd: no sys_time, wake_alarm" "$(((b >> 25 & 1) + (b >> 35 & 1)))" 0
is "udevd: keeps sys_admin, sys_module" "$(((b >> 21 & 1) + (b >> 16 & 1)))" 2
[ "$(readlink "/proc/$u/ns/mnt")" != "$(readlink /proc/self/ns/mnt)" ] && ok "udevd: private mount namespace" || bad "udevd: host mount namespace"
[ "$(readlink "/proc/$u/ns/uts")" != "$(readlink /proc/self/ns/uts)" ] && ok "udevd: private UTS namespace" || bad "udevd: host UTS namespace"
mount -t tmpfs late /mnt/late
grep -q ' /mnt/late ' "/proc/$u/mountinfo" && ok "udevd: sees a mount made later" || bad "udevd: doesn't see a mount made later"
udevadm trigger --action=add --settle
is "udevd: a rule's RUN+= runs" "$(cat /run/vm-rule-ran 2>/dev/null)" 0
udevadm info --query=property /sys/class/mem/null | grep -q '^VM_TEST=yes$' && ok "udevd: device in the database" || bad "udevd: device not in the database"

echo '--- rsyslog'
wait_for '[ -S /dev/log ]' || bad "no /dev/log"
r=$(pid_of rsyslogd)
is "rsyslog: cgroup" "$(cat "/proc/$r/cgroup")" "0::/system.slice/rsyslog.service"
is "rsyslog: no_new_privs" "$(status "$r" NoNewPrivs)" 1
is "rsyslog: seccomp" "$(status "$r" Seccomp)" 2
is "rsyslog: umask" "$(status "$r" Umask)" 0066
is "rsyslog: RLIMIT_NOFILE" "$(awk '/open files/ { print $4 ":" $5 }' "/proc/$r/limits")" 16384:16384
case $(top_mount "$r" /home) in ro) ok "rsyslog: /home read-only" ;; *) bad "rsyslog: /home $(top_mount "$r" /home)" ;; esac
# syslog starts before tmpfiles-setup, which would otherwise create /run/user.
is "rsyslog: /run/user read-only" "$(top_mount "$r" /run/user)" ro
is "rsyslog: /run/user's mode" "$(stat -c %a /run/user)" 755
is "rsyslog: /dev/log for everyone" "$(stat -c %a /dev/log)" 666
logger -p cron.info vm-cron-message
logger -p authpriv.info vm-auth-message
wait_for 'grep -qs vm-cron-message /var/log/cron && grep -qs vm-auth-message /var/log/secure && grep -qs "kernel: Linux version" /var/log/messages' || bad "rsyslog: a message didn't arrive"
is "rsyslog: new file private" "$(stat -c %a /var/log/cron)" 600
is "rsyslog: new file private, secure" "$(stat -c %a /var/log/secure)" 600

echo '--- earlyoom'
wait_for '[ -n "$(pid_of earlyoom)" ]' || bad "earlyoom didn't start"
e=$(pid_of earlyoom)
sleep 3
is "earlyoom: stays up" "$(pid_of earlyoom)" "$e"
is "earlyoom: user" "$(status "$e" Uid | tr -s '\t ' ' ')" "990 990 990 990"
is "earlyoom: groups" "$(status "$e" Gid | tr -s '\t ' ' ')" "990 990 990 990"
is "earlyoom: effective capabilities" "$(status "$e" CapEff)" 0000000000004020
is "earlyoom: bounding set" "$(status "$e" CapBnd)" 0000000000004020
is "earlyoom: no_new_privs" "$(status "$e" NoNewPrivs)" 1
is "earlyoom: seccomp" "$(status "$e" Seccomp)" 2
is "earlyoom: cgroup" "$(cat "/proc/$e/cgroup")" "0::/system.slice/earlyoom.service"
is "earlyoom: memory.max" "$(cat /sys/fs/cgroup/system.slice/earlyoom.service/memory.max 2>/dev/null)" 52428800
is "earlyoom: nice" "$(awk '{ print $19 }' "/proc/$e/stat")" -20
is "earlyoom: OOM score" "$(cat "/proc/$e/oom_score_adj")" -100
[ "$(readlink "/proc/$e/ns/net")" != "$(readlink /proc/self/ns/net)" ] && ok "earlyoom: private network" || bad "earlyoom: host network"
is "earlyoom: / read-only" "$(top_mount "$e" /)" ro
# tail keeps a line with no end in memory. Fed slowly enough for earlyoom's
# polling, or the kernel's OOM killer gets there first.
(
    while :; do
        printf '%01048576d' 0
        sleep 0.02
    done
) | tail &
hog=$!
wait_for '! kill -0 $hog 2>/dev/null' && ok "earlyoom: kills a root process under memory pressure" || bad "earlyoom: hog still running"
wait "$hog" 2>/dev/null
is "earlyoom: still the same process (not the kernel's OOM killer)" "$(pid_of earlyoom)" "$e"
kill -9 "$e"
# Until it has exec()ed: before that the runner has the old comm's name.
wait_for 'e2=$(pid_of earlyoom); [ -n "$e2" ] && [ "$e2" != "$e" ] && [ "$(status "$e2" Seccomp)" = 2 ]' || bad "earlyoom: not restarted"
e=$(pid_of earlyoom)
is "earlyoom: user after a restart" "$(status "$e" Uid | tr -s '\t ' ' ')" "990 990 990 990"
sleep 2
is "earlyoom: up after a restart" "$(pid_of earlyoom)" "$e"

echo '--- udevd fallback'
is "udevd: one try" "$(cut -d' ' -f1 /run/uinitconverter/udevd-tries)" 1
# A restart after a while is not a failure to start.
read -r _ last </run/uinitconverter/udevd-tries
echo "3 $((last - 120))" >/run/uinitconverter/udevd-tries
kill -9 "$u"
wait_for 'udevd_changed && [ "$(cut -d" " -f1 /run/uinitconverter/udevd-tries)" = 1 ]' || bad "udevd: no restart, tries $(cat /run/uinitconverter/udevd-tries)"
u=$(s6-svstat -p /service/udevd)
is "udevd: restarted with its sandbox" "$(status "$u" Seccomp)" 2
# Now a sandbox that can't be applied.
rm /usr/lib/systemd/system/systemd-udevd.service
kill -9 "$u"
wait_for '[ "$(cut -d" " -f1 /run/uinitconverter/udevd-tries)" = 4 ] && udevd_changed' || bad "udevd: no fallback, tries $(cat /run/uinitconverter/udevd-tries)"
u=$(s6-svstat -p /service/udevd)
is "udevd: fallback runs it without seccomp" "$(status "$u" Seccomp)" 0
is "udevd: fallback tries" "$(cut -d' ' -f1 /run/uinitconverter/udevd-tries)" 4
is "udevd: fallback OOM score" "$(cat "/proc/$u/oom_score_adj")" -1000
wait_for 's6-svstat /service/udevd | grep -q ready' && ok "udevd: fallback ready" || bad "udevd: fallback not ready"

echo '--- PrivateUsers='
mkdir -m 1777 /run/pu
pu() { sed -n "s/^$2 //p" "/run/pu/$1" 2>/dev/null; }
pu_run() { # command (run or confine) and unit: the probe's file appears
    rm -f "/run/pu/$2"
    case $1 in
        run) /usr/libexec/uinitconverter/initconvert-runner run -- "$2.service" >"/run/$2.log" 2>&1 & ;;
        confine) /usr/libexec/uinitconverter/initconvert-runner confine "$2.service" -- /usr/bin/pu-probe "/run/pu/$2" >"/run/$2.log" 2>&1 & ;;
    esac
    wait_for "[ -e /run/pu/$2 ]" || { bad "$2 ($1): no result: $(cat "/run/$2.log")"; return 1; }
}
host_ns=$(readlink /proc/self/ns/user)
for how in run confine; do
    pu_run "$how" pu-none
    is "PrivateUsers ($how): unset, the host's namespace" "$(pu pu-none ns)" "$host_ns"
    pu_run "$how" pu-no
    is "PrivateUsers=no ($how): the host's namespace" "$(pu pu-no ns)" "$host_ns"

    pu_run "$how" pu-user
    [ "$(pu pu-user ns)" != "$host_ns" ] && ok "PrivateUsers=yes, User= ($how): own namespace" || bad "PrivateUsers=yes, User= ($how): host's namespace"
    is "PrivateUsers=yes, User= ($how): uid" "$(pu pu-user uid)" 991
    is "PrivateUsers=yes, User= ($how): gid" "$(pu pu-user gid)" 991
    is "PrivateUsers=yes, User= ($how): uid_map" "$(pu pu-user uid_map)" "0 0 1,991 991 1"
    is "PrivateUsers=yes, User= ($how): gid_map" "$(pu pu-user gid_map)" "0 0 1,991 991 1"
    is "PrivateUsers=yes, User= ($how): setgroups denied" "$(pu pu-user setgroups)" deny
    # Set before the namespace; an unmapped group shows as nobody.
    is "PrivateUsers=yes, User= ($how): extra group kept, unmapped" "$(pu pu-user groups)" "991 65534"
    is "PrivateUsers=yes, User= ($how): bounding set applied in the namespace" "$(pu pu-user capbnd)" 0000000000000000

    pu_run "$how" pu-root
    is "PrivateUsers=yes, root ($how): uid_map" "$(pu pu-root uid_map)" "0 0 1"
    is "PrivateUsers=yes, root ($how): gid_map" "$(pu pu-root gid_map)" "0 0 1"
    is "PrivateUsers=yes, root ($how): no capabilities" "$(pu pu-root capeff)" 0000000000000000

    # All capabilities, but only in its own namespace.
    pu_run "$how" pu-caps
    [ "$(pu pu-caps capeff)" != 0000000000000000 ] && ok "PrivateUsers=yes, root ($how): has capabilities in the namespace" || bad "PrivateUsers=yes, root ($how): capeff $(pu pu-caps capeff)"
    is "PrivateUsers=yes, root ($how): they don't reach the host's mounts" "$(pu pu-caps mount)" refused

    pu_run "$how" pu-identity
    is "PrivateUsers=identity ($how): uid_map" "$(pu pu-identity uid_map)" "0 0 65536"
    is "PrivateUsers=identity ($how): setgroups denied" "$(pu pu-identity setgroups)" deny

    pu_run "$how" pu-full
    is "PrivateUsers=full ($how): uid_map" "$(pu pu-full uid_map)" "0 0 4294967295"
    is "PrivateUsers=full ($how): setgroups allowed" "$(pu pu-full setgroups)" allow
    is "PrivateUsers=full ($how): no host mounts" "$(pu pu-full mount)" refused

    # A mount stacked on / counts as a chroot to unshare(CLONE_NEWUSER): every
    # unit of this shape failed with EPERM on a real boot (colord, upower).
    pu_run "$how" pu-strict
    [ -n "$(pu pu-strict ns)" ] && [ "$(pu pu-strict ns)" != "$host_ns" ] && ok "PrivateUsers=yes, ProtectSystem=strict ($how): own namespace" || bad "PrivateUsers=yes, ProtectSystem=strict ($how): host's namespace"
    is "PrivateUsers=yes, ProtectSystem=strict ($how): uid_map" "$(pu pu-strict uid_map)" "0 0 1"
    is "PrivateUsers=yes, ProtectSystem=strict ($how): / read-only" "$(pu pu-strict rootfs)" ro
done
# unshare(CLONE_NEWUSER) fails in a chroot, so the order matters. Only `run`
# does RootDirectory=.
mkdir -p /pu-root
mount --rbind / /pu-root
/usr/libexec/uinitconverter/initconvert-runner run -- pu-chroot.service >/run/pu-chroot.log 2>&1 &
wait_for '[ -e /run/pu/pu-chroot ]' || bad "PrivateUsers= with RootDirectory=: no result: $(cat /run/pu-chroot.log)"
is "PrivateUsers= with RootDirectory=: chrooted" "$(pu pu-chroot chroot)" yes
is "PrivateUsers= with RootDirectory=: uid_map" "$(pu pu-chroot uid_map)" "0 0 1"
umount -l /pu-root

echo VM-DONE
sleep 1
echo o >/proc/sysrq-trigger
sleep 60
EOF
chmod +x "$R/init"
(cd "$R" && find . -print0 | sort -z | cpio --null -o -H newc -R 0:0 --quiet) >"$W/initrd.cpio"

kvm=()
[ -w /dev/kvm ] && kvm=(-enable-kvm -cpu host)
boot() {
    timeout 150 qemu-system-x86_64 "${kvm[@]}" -m 1024 -smp 2 -nodefaults -display none \
        -serial stdio -no-reboot -kernel "$KERNEL" -initrd "$W/initrd.cpio" \
        -append "console=ttyS0 quiet loglevel=1 panic=-1 $1" </dev/null >"$2" 2>&1 || true
    tr -d '\r' <"$2" >"$2.txt"
}
boot "" "$W/console.log"
boot "uinitconverter.udev=plain" "$W/console-plain.log"

fail=0
for l in "$W/console.log.txt" "$W/console-plain.log.txt"; do
    grep -aE '^(PASS|FAIL) ' "$l" || true
    grep -aq '^FAIL ' "$l" && fail=1
    grep -aq '^VM-DONE' "$l" || { echo "FAIL the VM didn't finish (see $l)"; fail=1; }
done
grep -aqE 'sending SIG(TERM|KILL) to process' "$W/console.log.txt" && echo "PASS earlyoom: said so" || { echo "FAIL earlyoom: nothing about the kill (see $W/console.log.txt)"; fail=1; }
echo "passed: $(cat "$W"/console*.txt | grep -ac '^PASS ')  failed: $(cat "$W"/console*.txt | grep -ac '^FAIL ')"
exit "$fail"
