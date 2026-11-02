#!/bin/bash
# A Fedora Workstation (GNOME) VM in QEMU, unprivileged, for tests that need a
# real boot, GDM and a GNOME session: the user-manager work first. Nothing is
# installed on the host; everything lives in DIR.
#
#   tests/gnome-vm.sh fetch DIR          # the Fedora Cloud image, checksum verified with gpgv
#   tests/gnome-vm.sh provision DIR      # base.qcow2: GNOME, GDM autologin as "tester"; powers off
#   tests/gnome-vm.sh start DIR [--keep] # boot a fresh overlay on base.qcow2 (--keep: the last one)
#   tests/gnome-vm.sh wait DIR           # until tester's GNOME session is up (or fail after 5 min)
#   tests/gnome-vm.sh check DIR          # print the session's state (logind, user manager, shell)
#   tests/gnome-vm.sh ssh DIR [CMD...]   # as tester, who has passwordless sudo
#   tests/gnome-vm.sh copy DIR FILE...   # into ~tester/in/
#   tests/gnome-vm.sh screenshot DIR PNG # the screen, through QMP
#   tests/gnome-vm.sh key DIR COMBO...   # keys as QEMU qcodes: meta_l, ret, ctrl-alt-t
#   tests/gnome-vm.sh type DIR TEXT      # letters, digits, space, - . /
#   tests/gnome-vm.sh stop DIR           # power off (kill after 60 s)
#
# Needs qemu-system-x86_64 and qemu-img, /dev/kvm, genisoimage, gpgv,
# ssh/ssh-keygen, python3 and fedora-gpg-keys. The VM gets 4 GiB and 4 CPUs
# (VM_MEM=, VM_CPUS=), user-mode networking, a VNC display on 127.0.0.1 (the
# port is in DIR/ports), and SSH forwarded from a free local port.
# `provision` downloads the GNOME packages inside the VM (about 2 GB); it
# takes a while.
set -euo pipefail

FEDORA=${VM_FEDORA:-45}
MIRROR=${VM_MIRROR:-https://download.fedoraproject.org/pub/fedora/linux/releases}
MEM=${VM_MEM:-4096}
CPUS=${VM_CPUS:-4}

usage() {
    sed -n '2,25s/^# \{0,1\}//p' "$0" >&2
    exit 2
}
die() { echo "gnome-vm: $*" >&2; exit 1; }
[ $# -ge 2 ] || usage
CMD=$1 D=$2
shift 2
mkdir -p "$D"
D=$(cd "$D" && pwd)

freeport() {
    python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
running() { [ -s "$D/qemu.pid" ] && kill -0 "$(cat "$D/qemu.pid")" 2>/dev/null; }
sshopts() {
    # shellcheck disable=SC2046 # the port file holds two words
    set -- $(cat "$D/ports")
    echo "-p $1 -i $D/id_ed25519 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=5"
}
vssh() {
    # shellcheck disable=SC2046
    ssh $(sshopts) tester@127.0.0.1 "$@"
}

# QEMU with DISK; extra arguments go on the end.
qemu() {
    local disk=$1
    shift
    [ -w /dev/kvm ] || die "/dev/kvm is not writable"
    qemu-system-x86_64 -enable-kvm -cpu host -m "$MEM" -smp "$CPUS" \
        -drive "file=$disk,if=virtio,discard=unmap" \
        -device virtio-vga -device virtio-rng-pci \
        -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$SSHPORT-:22" \
        -serial "file:$D/serial.log" -qmp "unix:$D/qmp.sock,server,nowait" \
        "$@"
}

# download.fedoraproject.org sends each request to some mirror, and not
# every mirror has every file: try again (the signed CHECKSUM vouches for
# whatever mirror the image came from).
get() {
    local n
    for n in 1 2 3 4 5 6; do
        curl -fsSL "$@" && return 0
        echo "gnome-vm: download failed (try $n), again" >&2
        sleep 2
    done
    return 1
}

fetch() {
    local base="$MIRROR/$FEDORA/Cloud/x86_64/images" sum img key
    sum=$(get "$base/" | grep -o -E "Fedora-Cloud-$FEDORA-[0-9.]+-x86_64-CHECKSUM" | sort -u | tail -1)
    [ -n "$sum" ] || die "no CHECKSUM file under $base"
    get -o "$D/CHECKSUM" "$base/$sum"
    key=/usr/share/pki/rpm-gpg/RPM-GPG-KEY-fedora-$FEDORA-primary
    [ -r "$key" ] || key=/usr/share/pki/rpm-gpg/RPM-GPG-KEY-$FEDORA-fedora
    [ -r "$key" ] || die "no Fedora $FEDORA key (install fedora-gpg-keys)"
    gpg --dearmor <"$key" >"$D/fedora.gpg"
    gpgv --keyring "$D/fedora.gpg" "$D/CHECKSUM" 2>"$D/gpgv.log" || { cat "$D/gpgv.log" >&2; die "CHECKSUM signature not valid"; }
    img=$(grep -o -E "Fedora-Cloud-Base-Generic-$FEDORA-[0-9.]+\.x86_64\.qcow2" "$D/CHECKSUM" | head -1)
    [ -n "$img" ] || die "no Generic qcow2 in CHECKSUM"
    if [ ! -s "$D/$img" ]; then
        get -o "$D/$img.part" "$base/$img"
        mv "$D/$img.part" "$D/$img"
    fi
    (cd "$D" && grep -E "^SHA256 \($img\)" CHECKSUM | sha256sum -c -) || die "checksum mismatch for $img"
    ln -sf "$img" "$D/cloud.qcow2"
    echo "gnome-vm: $D/$img verified"
}

provision() {
    [ -e "$D/cloud.qcow2" ] || die "run fetch first"
    [ -e "$D/id_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -C gnome-vm -f "$D/id_ed25519"
    local seed=$D/seed
    rm -rf "$seed"
    mkdir -p "$seed"
    printf 'instance-id: gnome-vm-%s\nlocal-hostname: gnome-vm\n' "$(date +%s)" >"$seed/meta-data"
    cat >"$seed/user-data" <<EOF
#cloud-config
users:
  - name: tester
    gecos: GNOME test user
    groups: [wheel]
    sudo: "ALL=(ALL) NOPASSWD:ALL"
    lock_passwd: false
    ssh_authorized_keys:
      - $(cat "$D/id_ed25519.pub")
chpasswd:
  expire: false
  users:
    - {name: tester, password: tester, type: text}
ssh_pwauth: false
write_files:
  - path: /etc/gdm/custom.conf
    content: |
      [daemon]
      AutomaticLoginEnable=True
      AutomaticLogin=tester
  - path: /etc/dconf/db/local.d/00-gnome-vm
    content: |
      [org/gnome/shell]
      welcome-dialog-last-shown-version='999'
  - path: /home/tester/.config/gnome-initial-setup-done
    content: "yes"
    owner: tester:tester
    defer: true
runcmd:
  - [dnf, -y, install, "@workstation-product-environment"]
  - [dconf, update]
  - [systemctl, set-default, graphical.target]
  - [touch, /var/lib/gnome-vm-provisioned]
power_state:
  mode: poweroff
  condition: true
  timeout: 30
EOF
    genisoimage -quiet -output "$D/seed.iso" -volid cidata -joliet -rock "$seed/user-data" "$seed/meta-data"
    rm -f "$D/base.qcow2" "$D/run.qcow2"
    qemu-img create -q -f qcow2 -F qcow2 -b "$(readlink -f "$D/cloud.qcow2")" "$D/base.qcow2" 40G
    SSHPORT=$(freeport)
    echo "gnome-vm: provisioning (serial log: $D/serial.log); this takes a while"
    qemu "$D/base.qcow2" -display none -drive "file=$D/seed.iso,media=cdrom,readonly=on" &
    local pid=$! t=0
    while kill -0 "$pid" 2>/dev/null; do
        if [ $t -ge 5400 ]; then
            kill "$pid"
            die "provisioning not done after 90 min (see $D/serial.log)"
        fi
        sleep 10
        t=$((t + 10))
    done
    wait "$pid" || die "provisioning QEMU failed (see $D/serial.log)"
    mv "$D/serial.log" "$D/provision.log"
    # Cloud-init's power_state also powers off after a failed runcmd: check.
    start >/dev/null
    local ok=0
    for _ in $(seq 60); do vssh true 2>/dev/null && break; sleep 5; done
    vssh test -e /var/lib/gnome-vm-provisioned && ok=1
    stop >/dev/null
    rm -f "$D/run.qcow2"
    [ $ok = 1 ] || die "provisioning didn't finish: no /var/lib/gnome-vm-provisioned (see $D/provision.log)"
    echo "gnome-vm: $D/base.qcow2 ready"
}

start() {
    [ -e "$D/base.qcow2" ] || die "run provision first"
    running && die "already running (pid $(cat "$D/qemu.pid"))"
    if [ "${1:-}" != --keep ] || [ ! -e "$D/run.qcow2" ]; then
        rm -f "$D/run.qcow2"
        qemu-img create -q -f qcow2 -F qcow2 -b "$D/base.qcow2" "$D/run.qcow2"
    fi
    SSHPORT=$(freeport)
    local vnc
    vnc=$(python3 -c 'import socket
for n in range(10, 100):
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", 5900 + n)); print(n); break
    except OSError:
        pass')
    echo "$SSHPORT $((5900 + vnc))" >"$D/ports"
    rm -f "$D/qmp.sock"
    : >"$D/serial.log"
    qemu "$D/run.qcow2" -display none -vnc "127.0.0.1:$vnc" -daemonize -pidfile "$D/qemu.pid"
    echo "gnome-vm: started; ssh port $SSHPORT, VNC 127.0.0.1:$((5900 + vnc))"
}

wait_session() {
    running || die "not running"
    local t=0
    until vssh true 2>/dev/null; do
        t=$((t + 5)); [ $t -lt 300 ] || die "no SSH after 5 min"; sleep 5
    done
    # Active: the session is on the screen (fgconsole), not only running.
    until vssh 'pgrep -u tester -x gnome-shell >/dev/null && loginctl show-user tester -p State --value | grep -qx active && s=$(loginctl show-user tester -p Display --value) && [ -n "$s" ] && loginctl show-session "$s" -p Active --value | grep -qx yes'; do
        t=$((t + 5)); [ $t -lt 300 ] || die "no GNOME session for tester after 5 min"; sleep 5
    done
    echo "gnome-vm: tester's GNOME session is up"
}

check() {
    vssh 'set -x
uname -r; cat /proc/1/comm; getenforce
loginctl list-sessions --no-legend
pgrep -a -u tester -x gnome-shell
pgrep -a -u tester -f "systemd --user|uinitconverter" | head
XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user is-system-running
XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user --failed --no-legend' 2>&1
}

# QMP commands, one JSON object per argument; fails on an error reply.
qmp() {
    python3 - "$D/qmp.sock" "$@" <<'PY2'
import json, socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
f = s.makefile("rw")
def send(c):
    f.write(json.dumps(c) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            return r
json.loads(f.readline())  # greeting
send({"execute": "qmp_capabilities"})
for a in sys.argv[2:]:
    r = send(json.loads(a))
    if "error" in r:
        sys.exit("qmp: %s: %s" % (a, r["error"]))
PY2
}

screenshot() {
    local out=${1:?screenshot needs a file name}
    out=$(cd "$(dirname "$out")" && pwd)/$(basename "$out")
    qmp "{\"execute\": \"screendump\", \"arguments\": {\"filename\": \"$out\", \"format\": \"png\"}}"
    echo "gnome-vm: $out"
}

# Key combinations as QEMU qcodes joined by "-": meta_l, ret, ctrl-alt-t.
key() {
    local combo keys
    for combo in "$@"; do
        keys=$(echo "$combo" | tr '-' '\n' | sed 's/.*/{"type": "qcode", "data": "&"}/' | paste -sd,)
        qmp "{\"execute\": \"send-key\", \"arguments\": {\"keys\": [$keys]}}"
        sleep 0.1
    done
}

# Text: letters, digits, space, - . / only.
typetext() {
    local text=$1 c q i
    for ((i = 0; i < ${#text}; i++)); do
        c=${text:i:1}
        case $c in
            [a-z0-9]) q=$c ;;
            [A-Z]) q="shift-$(echo "$c" | tr 'A-Z' 'a-z')" ;;
            ' ') q=spc ;; '-') q=minus ;; '.') q="dot" ;; '/') q=slash ;;
            *) die "typetext: can't type '$c'" ;;
        esac
        key "$q"
    done
}

stop() {
    running || { echo "gnome-vm: not running"; return 0; }
    vssh sudo systemctl poweroff 2>/dev/null || vssh sudo poweroff 2>/dev/null || true
    local t=0
    while running && [ $t -lt 60 ]; do sleep 1; t=$((t + 1)); done
    if running; then
        kill "$(cat "$D/qemu.pid")"
        echo "gnome-vm: killed after 60 s"
    fi
    rm -f "$D/qemu.pid"
}

case $CMD in
    fetch) fetch ;;
    provision) provision ;;
    start) start "$@" ;;
    wait) wait_session ;;
    check) check ;;
    ssh) vssh "$@" ;;
    copy)
        vssh mkdir -p in
        # shellcheck disable=SC2046
        scp $(sshopts | sed 's/-p /-P /') "$@" tester@127.0.0.1:in/
        ;;
    screenshot) screenshot "$@" ;;
    key) key "$@" ;;
    type) typetext "$*" ;;
    stop) stop ;;
    *) usage ;;
esac
