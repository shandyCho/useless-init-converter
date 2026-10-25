#!/bin/bash
# Boot the s6 initramfs (dist/initramfs, setup-fedora.sh --boot-only-s6) in
# QEMU, unprivileged: a LUKS2 disk with a btrfs root in subvol=root, opened
# with a passphrase typed on the serial console.
#
#   tests/initramfs.sh WORKDIR [KVER]
#
# Needs a readable /boot/vmlinuz-KVER (default: the running kernel), QEMU,
# /dev/kvm (else very slow) and mdevd: installed, or set MDEVD_BIN to the
# directory holding mdevd and mdevd-coldplug (e.g. an RPM unpacked by hand).
set -euo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
W=${1:?usage: tests/initramfs.sh WORKDIR [KVER]}
KVER=${2:-$(uname -r)}
KERNEL=/boot/vmlinuz-$KVER
PASS=sdcompat-test
MK=$TOP/dist/initramfs/mkinitramfs
CONSOLE=$TOP/tests/qemu-console.py
export SDCOMPAT_INITRAMFS_PATH=${MDEVD_BIN:+$MDEVD_BIN:}/usr/bin:/usr/sbin
# The test's own module list, not this host's /etc/sdcompat one.
export SDCOMPAT_INITRAMFS_MODULES=/dev/null

[ -r "$KERNEL" ] || { echo "$KERNEL not readable" >&2; exit 1; }
rm -rf "$W"
mkdir -p "$W/prep"
pass=0 fail=0
ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1 (see $2)"; fail=$((fail + 1)); }

kvm=()
[ -w /dev/kvm ] && kvm=(-enable-kvm -cpu host)
# The QEMU command line for one boot of the test kernel (add -initrd,
# -drive and -append).
QEMU=(qemu-system-x86_64 "${kvm[@]}" -m 1024 -smp 2 -nodefaults -display none
    -serial stdio -no-reboot -kernel "$KERNEL")

# --- The test root's init: report what it sees, then power off.
cat >"$W/prep/test-init" <<'EOF'
#!/bin/bash
root="" run=no
while read -r _ _ _ r mp _; do
    [ "$mp" = / ] && root=$r
    [ "$mp" = /run ] && run=yes
done </proc/self/mountinfo
procs=""
for p in /proc/[0-9]*; do
    # Kernel threads have no command line (/proc files show size 0: read).
    c=$(tr -d '\0' 2>/dev/null <"$p/cmdline") || continue
    [ -n "$c" ] && procs="$procs $(cat "$p/comm")"
done
udevdb=""
for f in /run/udev/data/b*; do
    [ -e "$f" ] && grep -q '^E:DM_UDEV_PRIMARY_SOURCE_FLAG=1$' "$f" && udevdb="$udevdb ${f##*/}"
done
echo "S6-INITRAMFS-TEST pid=$$ args=[$*] root=$root run=$run procs=[${procs# }] udevdb=[${udevdb# }]"
# sysrq's message would land in the middle of the line above.
sleep 1
echo o >/proc/sysrq-trigger
sleep 30
EOF

# --- Prep boot: make the disk (LUKS2, btrfs, subvol root holding test-init).
cat >"$W/prep/init" <<EOF
#!/bin/bash
export PATH=/usr/bin DM_DISABLE_UDEV=1
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mount -t tmpfs tmpfs /run
exec </dev/console >/dev/console 2>&1
set -e
mkdir -p /run/cryptsetup /mnt
printf %s '$PASS' | cryptsetup luksFormat -q --type luks2 --pbkdf argon2id \\
    --pbkdf-memory 65536 --iter-time 200 --key-file - /dev/vda
printf %s '$PASS' | cryptsetup open --key-file - /dev/vda prep
mkfs.btrfs -q /dev/mapper/prep
mount /dev/mapper/prep /mnt
btrfs -q subvolume create /mnt/root
cp -a /usr /mnt/root/
for l in bin sbin lib lib64; do cp -a /\$l /mnt/root/; done
mkdir -p /mnt/root/dev /mnt/root/proc /mnt/root/sys /mnt/root/run
install -m 0755 /test-init /mnt/root/usr/bin/init-selinux
echo "PREP luks=\$(blkid -s UUID -o value /dev/vda) root=\$(blkid -s UUID -o value /dev/mapper/prep)"
umount /mnt
cryptsetup close prep
echo PREP-DONE
echo o >/proc/sysrq-trigger
sleep 30
EOF
chmod 755 "$W/prep/init" "$W/prep/test-init"
"$MK" -k "$KVER" -o "$W/prep.img" --source "$W/prep" --no-microcode \
    --add "$(type -P mkfs.btrfs)" --add "$(type -P install)" --add "$(type -P cp)" --add "$(type -P tr)" --add "$(type -P grep)" >/dev/null
"$MK" -k "$KVER" -o "$W/s6.img" >/dev/null
truncate -s 512M "$W/disk.img"
if python3 -I "$CONSOLE" "$W/prep.log" 300 "wait:PREP-DONE" -- \
    "${QEMU[@]}" -initrd "$W/prep.img" -drive "file=$W/disk.img,if=virtio,format=raw" \
    -append "console=ttyS0 panic=-1 quiet"; then
    ok "prep: LUKS2 + btrfs subvol=root test disk"
else
    bad "prep" "$W/prep.log"
    exit 1
fi
luks=$(sed -n 's/^PREP luks=\([^ ]*\) .*/\1/p' "$W/prep.log" | tr -d '\r')
root=$(sed -n 's/^PREP .* root=\([^ \r]*\).*/\1/p' "$W/prep.log" | tr -d '\r')
cmdline="console=ttyS0 panic=-1 quiet root=UUID=$root rootflags=subvol=root rootflags=subvol=root ro rd.luks.uuid=luks-$luks init=/usr/sbin/init-selinux"
disk=(-drive "file=$W/disk.img,if=virtio,format=raw")

# boot NAME APPEND [DISK...] -- STEP...: boot the s6 initramfs once.
boot() {
    local name=$1 append=$2 d=() log
    log=$W/$(printf %s "$1" | tr -c 'a-zA-Z0-9.' '-').log
    shift 2
    while [ "$1" != -- ]; do d+=("$1"); shift; done
    shift
    if python3 -I "$CONSOLE" "$log" 120 "$@" -- \
        "${QEMU[@]}" -initrd "$W/s6.img" "${d[@]}" -append "$append"; then
        ok "$name"
    else
        bad "$name" "$log"
    fi
}
result='S6-INITRAMFS-TEST pid=1 args=\[testarg\] root=/root run=yes procs=\[init-selinux\] udevdb=\[b25[0-9]:0\]'

boot "passphrase, LUKS2 + btrfs subvol, switch_root to init with its args" \
    "$cmdline testarg" "${disk[@]}" -- \
    "wait:Enter passphrase" "sleep:1" "send:$PASS" "wait:$result"
# No udev makes no /dev/disk links: rootfs looks the path up as UUID=.
boot "root=/dev/disk/by-uuid/... without udev links" \
    "${cmdline/root=UUID=/root=/dev/disk/by-uuid/} testarg" "${disk[@]}" -- \
    "wait:Enter passphrase" "sleep:1" "send:$PASS" "wait:$result"
# nvme is a module (virtio_blk is built in): only mdevd loading it from the
# device's modalias makes the disk appear, as on a laptop's NVMe root.
boot "NVMe disk: mdevd loads the nvme module" \
    "$cmdline testarg" -drive "file=$W/disk.img,if=none,id=d0,format=raw" \
    -device nvme,drive=d0,serial=sdc0 -- \
    "wait:Enter passphrase for /dev/nvme0n1" "sleep:1" "send:$PASS" "wait:$result"
boot "wrong passphrase three times, rescue shell, retry after exit" \
    "$cmdline testarg" "${disk[@]}" -- \
    "wait:Enter passphrase" "sleep:1" "send:wrong" "wait:Enter passphrase" "sleep:1" "send:wrong" \
    "wait:Enter passphrase" "sleep:1" "send:wrong" "wait:rescue shell" "send:exit" \
    "wait:Enter passphrase" "sleep:1" "send:$PASS" "wait:$result"
boot "rd.break: shell before switching, then boots" \
    "$cmdline testarg rd.break" "${disk[@]}" -- \
    "wait:Enter passphrase" "sleep:1" "send:$PASS" "wait:rd.break: shell" "send:exit" "wait:$result"
boot "no disk: times out into the rescue shell" \
    "$cmdline sdcompat.initrd.timeout=2" -- \
    "wait:LUKS device UUID=$luks not found" "wait:rescue shell" "send:echo o >/proc/sysrq-trigger"

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
