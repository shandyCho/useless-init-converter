#!/bin/bash
# Set up a Fedora (Workstation/GNOME) system to boot with s6-linux-init +
# s6-rc + sdcompat instead of systemd as PID 1.
#
# The normal systemd boot entry stays the default and is left working:
# this script only ADDS a GRUB entry "Fedora (s6 + sdcompat)". Packages and
# settings it changes are listed in the plan it prints before doing anything.
#
#   sudo dist/setup-fedora.sh            # interactive: shows the plan, asks
#   sudo dist/setup-fedora.sh --yes      # no questions
#   dist/setup-fedora.sh --dry-run       # print every command, change nothing
#   sudo dist/setup-fedora.sh --uninstall
#
# Options:
#   --enforcing     do not add enforcing=0 to the s6 boot entry (default: SELinux
#                   permissive on that entry only, until you have a local policy)
#   --no-grub       do not add the boot entry
#   --build-s6      build skalibs/execline/s6/s6-rc/s6-linux-init from source
#                   even if Fedora packages exist
#   --rebuild       rebuild sdcompat even if target/release/sdcompat exists
#   --no-journal    no systemd-journald: rsyslog + per-service s6-log directories
#                   (journalctl and the log lines in systemctl status won't work)
set -euo pipefail

REPO=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
DIST=$REPO/dist
TITLE="Fedora (s6 + sdcompat)"
S6LI_BASE=/etc/s6-linux-init/current
LIBEXEC=/usr/libexec/sdcompat
SHARE=/usr/share/sdcompat
BACKUP=/var/lib/sdcompat/setup-backup/$(date +%Y%m%d-%H%M%S)

# skarnet versions known to work together (same set as Artix's s6 stack).
SKALIBS_VER=2.15.1.0
EXECLINE_VER=2.9.9.2
S6_VER=2.15.1.0
S6RC_VER=0.7.0.0
S6LI_VER=1.2.0.2

DRY=0 YES=0 ENFORCING=0 GRUB=1 BUILD_S6=0 REBUILD=0 UNINSTALL=0 JOURNAL=1
KI_PLUGIN=/etc/kernel/install.d/90-sdcompat.install
for a in "$@"; do
    case $a in
        --dry-run) DRY=1 ;;
        --yes|-y) YES=1 ;;
        --enforcing) ENFORCING=1 ;;
        --no-grub) GRUB=0 ;;
        --build-s6) BUILD_S6=1 ;;
        --rebuild) REBUILD=1 ;;
        --uninstall) UNINSTALL=1 ;;
        --no-journal) JOURNAL=0 ;;
        -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
        *) echo "unknown option: $a" >&2; exit 2 ;;
    esac
done

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Run a command, or print it in --dry-run mode.
run() {
    if [ "$DRY" = 1 ]; then
        printf '  + %s\n' "$*"
    else
        "$@"
    fi
}

# Write stdin to a file (backing up an existing one), or show it in --dry-run.
put() {
    local dest=$1 mode=${2:-0644}
    if [ "$DRY" = 1 ]; then
        printf '  + write %s (mode %s)\n' "$dest" "$mode"
        cat >/dev/null
        return
    fi
    backup "$dest"
    mkdir -p "$(dirname "$dest")"
    cat >"$dest"
    chmod "$mode" "$dest"
}

backup() {
    local f=$1
    [ -e "$f" ] || return 0
    [ "$DRY" = 1 ] && { printf '  + backup %s\n' "$f"; return; }
    mkdir -p "$BACKUP$(dirname "$f")"
    cp -a "$f" "$BACKUP$f"
}

confirm() {
    [ "$YES" = 1 ] || [ "$DRY" = 1 ] && return 0
    read -r -p "$1 [y/N] " ans
    case $ans in [yY]*) return 0 ;; *) echo "aborted"; exit 1 ;; esac
}

check_system() {
    if [ "$DRY" = 0 ] && [ "$(id -u)" != 0 ]; then
        die "run as root (or use --dry-run)"
    fi
    . /etc/os-release
    if [ "${ID:-}" != fedora ]; then
        if [ "$DRY" = 1 ] && [ "${SDCOMPAT_SETUP_ANY_OS:-}" = 1 ]; then
            warn "not Fedora ($ID); dry run only"
        else
            die "this script is for Fedora (found ID=${ID:-unknown})"
        fi
    fi
    [ -f "$REPO/Cargo.toml" ] || die "run from the sdcompat source tree ($REPO)"
}

# ---------------------------------------------------------------- uninstall

uninstall() {
    say "Removing the s6 boot entry and sdcompat files"
    confirm "Remove the '$TITLE' boot entry, $S6LI_BASE and sdcompat's files?"
    remove_grub_entry
    run rm -f "$KI_PLUGIN" /etc/sdcompat/boot-options
    run rm -rf "$S6LI_BASE"
    if command -v rpm >/dev/null && rpm -q sdcompat >/dev/null 2>&1; then
        echo "  sdcompat is installed as an RPM; remove it with: dnf remove sdcompat"
    else
        run rm -rf "$LIBEXEC" "$SHARE" /usr/bin/sdcompat /usr/sbin/init-selinux \
            /usr/share/dbus-1/system.d/sdcompat.conf
    fi
    echo "  Kept: /etc/sdcompat (your configuration), /var/lib/sdcompat, /var/log/sdcompat,"
    echo "  installed packages, and enabled units (all harmless for the systemd boot)."
    exit 0
}

remove_grub_entry() {
    local f
    if [ -d /boot/loader/entries ]; then
        for f in /boot/loader/entries/*.conf; do
            [ -e "$f" ] || continue
            if grep -q "^title $TITLE" "$f"; then
                run rm -f "$f"
            fi
        done
    elif command -v grubby >/dev/null; then
        run grubby --remove-kernel="TITLE=$TITLE" || true
    fi
}

# ---------------------------------------------------------------- steps

install_packages() {
    say "Installing Fedora packages"
    local pkgs=(dbus-daemon earlyoom chrony NetworkManager
                policycoreutils-python-utils grubby curl tar gcc make)
    [ "$JOURNAL" = 1 ] || pkgs+=(rsyslog)
    [ -x "$REPO/target/release/sdcompat" ] && [ "$REBUILD" = 0 ] || pkgs+=(cargo rust)
    if ! rpm -q gdm gnome-shell >/dev/null 2>&1; then
        warn "gdm/gnome-shell are not installed. For a full desktop run:"
        warn "  dnf group install workstation-product-environment"
    fi
    run dnf install -y "${pkgs[@]}"
}

install_skarnet() {
    say "Installing skalibs, execline, s6, s6-rc, s6-linux-init"
    local missing=()
    if [ "$BUILD_S6" = 0 ]; then
        local p
        for p in skalibs execline s6 s6-rc s6-linux-init; do
            if ! rpm -q "$p" >/dev/null 2>&1 && ! dnf -q list --available "$p" >/dev/null 2>&1; then
                missing+=("$p")
            fi
        done
        if [ ${#missing[@]} = 0 ]; then
            run dnf install -y skalibs execline s6 s6-rc s6-linux-init
            return
        fi
        warn "not packaged for this Fedora release: ${missing[*]}; building all five from source"
    fi
    # Building into /usr must not overwrite files owned by installed RPMs, and
    # the five must be versions that fit together.
    local owned=()
    for p in skalibs execline s6 s6-rc s6-linux-init; do
        rpm -q "$p" >/dev/null 2>&1 && owned+=("$p")
    done
    if [ ${#owned[@]} -gt 0 ]; then
        die "already installed as RPMs: ${owned[*]}. Remove them (dnf remove ${owned[*]}) so all five are built as one matching set, or package the missing ones."
    fi
    # Build in dependency order into /usr (statically linked against skalibs).
    local work=/tmp/sdcompat-skarnet-build
    [ "$DRY" = 1 ] || work=$(mktemp -d)
    local spec
    for spec in "skalibs $SKALIBS_VER" "execline $EXECLINE_VER" "s6 $S6_VER" \
                "s6-rc $S6RC_VER" "s6-linux-init $S6LI_VER"; do
        set -- $spec
        local name=$1 ver=$2
        echo "  building $name $ver"
        run curl -fsSL -o "$work/$name.tar.gz" "https://skarnet.org/software/$name/$name-$ver.tar.gz"
        run tar -xzf "$work/$name.tar.gz" -C "$work"
        if [ "$DRY" = 0 ]; then
            (cd "$work/$name-$ver" && ./configure --prefix=/usr >/dev/null && make -j"$(nproc)" >/dev/null && make install >/dev/null)
        else
            printf '  + (cd %s && ./configure --prefix=/usr && make && make install)\n' "$work/$name-$ver"
        fi
    done
    [ "$DRY" = 1 ] || rm -rf "$work"
}

install_sdcompat() {
    say "Installing sdcompat"
    if rpm -q sdcompat >/dev/null 2>&1; then
        echo "  sdcompat RPM is installed, using it"
        return
    fi
    if [ ! -x "$REPO/target/release/sdcompat" ] || [ "$REBUILD" = 1 ]; then
        local builder=${SUDO_USER:-root}
        if [ "$builder" != root ]; then
            run su "$builder" -c "cd '$REPO' && cargo build --release"
        else
            run sh -c "cd '$REPO' && cargo build --release"
        fi
    fi
    run install -Dm0755 "$REPO/target/release/sdcompat" "$LIBEXEC/sdcompat"
    run install -Dm0755 "$DIST/early" "$LIBEXEC/early"
    run ln -sfn ../libexec/sdcompat/sdcompat /usr/bin/sdcompat
    run install -Dm0644 "$DIST/dbus/sdcompat.conf" /usr/share/dbus-1/system.d/sdcompat.conf
    run install -Dm0755 "$DIST/s6-linux-init/init-selinux" /usr/sbin/init-selinux
    run mkdir -p "$SHARE/s6-linux-init"
    run rm -rf "$SHARE/s6-linux-init/skel"
    run cp -a "$DIST/s6-linux-init/skel" "$SHARE/s6-linux-init/skel"
    run install -Dm0644 "$DIST/rsyslog.conf" "$SHARE/rsyslog.conf"
}

configure_sdcompat() {
    say "Configuring /etc/sdcompat"
    if [ -e /etc/sdcompat/sdcompat.conf ]; then
        echo "  keeping existing /etc/sdcompat/sdcompat.conf"
    elif [ "$JOURNAL" = 1 ]; then
        put /etc/sdcompat/sdcompat.conf <"$DIST/sdcompat.conf"
    else
        { cat "$DIST/sdcompat.conf"; printf '\n# set by setup-fedora.sh --no-journal\nJOURNAL=no\n'; } |
            put /etc/sdcompat/sdcompat.conf
    fi
    [ "$JOURNAL" = 1 ] || put /etc/sdcompat/rsyslog.conf <"$DIST/rsyslog.conf"

    # Hand-written s6 services: add missing ones, never overwrite your edits.
    run mkdir -p /etc/sdcompat/s6
    local d name
    for d in "$DIST"/s6/*/; do
        name=$(basename "$d")
        if [ -e "/etc/sdcompat/s6/$name" ]; then
            if ! diff -rq "$d" "/etc/sdcompat/s6/$name" >/dev/null 2>&1; then
                echo "  keeping your modified /etc/sdcompat/s6/$name (new version: $d)"
            fi
        else
            run cp -a "$d" "/etc/sdcompat/s6/$name"
        fi
    done
    # With journald, it owns /dev/log; the s6-only rsyslog is not started.
    if [ "$JOURNAL" = 1 ]; then
        run rm -f /etc/sdcompat/s6/sdcompat-sysinit/contents.d/syslog
    elif [ ! -e /etc/sdcompat/s6/sdcompat-sysinit/contents.d/syslog ]; then
        run touch /etc/sdcompat/s6/sdcompat-sysinit/contents.d/syslog
    fi
    run mkdir -p /var/lib/sdcompat /var/log/sdcompat /var/lib/rsyslog
}

enable_units() {
    say "Enabling units (these are also normal on the systemd boot)"
    local u
    for u in chronyd.service NetworkManager.service; do
        run systemctl --no-reload enable "$u"
    done
    if rpm -q gdm >/dev/null 2>&1; then
        run systemctl --no-reload enable gdm.service
        run systemctl --no-reload set-default graphical.target
    fi
}

build_database() {
    say "Translating units into an s6-rc database"
    if [ "$DRY" = 1 ]; then
        printf '  + sdcompat reload --no-update\n'
        return
    fi
    "$LIBEXEC/sdcompat" reload --no-update
    local report=/var/lib/sdcompat/current/report.txt
    echo "  report: $report"
    grep -E '^(translated|skipped|cycles broken):' "$report" | sed 's/^/  /'
    local n
    n=$(grep -c 'warning:' "$report" || true)
    echo "  $n warnings (mostly sandboxing directives that are not enforced)"
}

setup_s6_linux_init() {
    say "Generating s6-linux-init in $S6LI_BASE"
    local tmp=/tmp/sdcompat-s6li
    [ "$DRY" = 1 ] || tmp=$(mktemp -d)
    run s6-linux-init-maker \
        -c "$S6LI_BASE" \
        -f "$SHARE/s6-linux-init/skel" \
        -p /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin \
        -1 -t 2 -D default \
        -G "/usr/sbin/agetty tty9 linux" \
        "$tmp/current"
    if [ -e "$S6LI_BASE" ]; then
        run mkdir -p "$BACKUP/etc/s6-linux-init"
        run mv "$S6LI_BASE" "$BACKUP$S6LI_BASE"
    fi
    run mkdir -p /etc/s6-linux-init
    run cp -a "$tmp/current" "$S6LI_BASE"
    [ "$DRY" = 1 ] || rm -rf "$tmp"
    echo "  rescue: an early getty runs on tty9 (Ctrl+Alt+F9) even if stage 2 fails"
}

setup_selinux() {
    say "SELinux labels"
    if ! command -v semanage >/dev/null && [ "$DRY" = 0 ]; then
        warn "semanage missing; skipping labels"
        return
    fi
    # s6-linux-init's stage 1 must run as init_t, like systemd.
    if ! semanage fcontext -l 2>/dev/null | grep -q "^$S6LI_BASE/bin/init "; then
        run semanage fcontext -a -t init_exec_t "$S6LI_BASE/bin/init"
    fi
    run restorecon -R "$S6LI_BASE" "$LIBEXEC" /usr/sbin/init-selinux /etc/sdcompat \
        /var/lib/sdcompat /var/log/sdcompat /usr/share/dbus-1/system.d/sdcompat.conf
}

boot_args() {
    # plymouth's splash would keep the display (plymouth-quit.service is a
    # systemd-only unit), so the s6 entry boots without it and shows messages.
    local args="init=/usr/sbin/init-selinux plymouth.enable=0 rd.plymouth=0"
    [ "$ENFORCING" = 1 ] || args="$args enforcing=0"
    echo "$args"
}

add_grub_entry() {
    [ "$GRUB" = 1 ] || return 0
    say "Adding boot entry: $TITLE"
    local args
    args=$(boot_args)
    printf '# Extra kernel options for the "%s" boot entries.\n%s\n' "$TITLE" "$args" |
        put /etc/sdcompat/boot-options
    local kernel
    if [ "$DRY" = 1 ] && ! command -v grubby >/dev/null; then
        kernel=/boot/vmlinuz-KVER
    else
        kernel=$(grubby --default-kernel)
    fi
    local kver=${kernel#/boot/vmlinuz-}
    if [ -d /boot/loader/entries ] || [ "$DRY" = 1 ]; then
        # The kernel-install plugin copies the BLS entry for each kernel,
        # now and on every future kernel update.
        run install -Dm0755 "$DIST/kernel-install/90-sdcompat.install" "$KI_PLUGIN"
        remove_grub_entry
        run "$KI_PLUGIN" add "$kver"
    else
        remove_grub_entry
        run grubby --add-kernel="$kernel" --initrd="/boot/initramfs-$kver.img" \
            --copy-default --title="$TITLE $kver" --args="$args"
        warn "no BLS entries: future kernels will not get an s6 entry automatically"
    fi
    echo "  The systemd entry stays the default. Pick '$TITLE' in the GRUB menu."
}

print_plan() {
    cat <<EOF

This will set up booting Fedora with s6-linux-init + s6-rc + sdcompat:

  * install: dbus-daemon earlyoom chrony NetworkManager$([ "$JOURNAL" = 1 ] || echo " rsyslog"), the skarnet tools
    (from Fedora if packaged, else built from skarnet.org into /usr), sdcompat
  * write:   /etc/sdcompat/ (config, s6 services, boot options),
             $S6LI_BASE, /usr/sbin/init-selinux, $LIBEXEC
  * enable:  chronyd, NetworkManager, gdm; default target graphical.target
  * SELinux: label $S6LI_BASE/bin/init as init_exec_t
  * logs:    $([ "$JOURNAL" = 1 ] && echo "systemd-journald as an s6 service (journalctl works)" || echo "rsyslog + s6-log directories, no journal")
  * GRUB:    add "$TITLE" with init=/usr/sbin/init-selinux, no splash$([ "$ENFORCING" = 1 ] || echo ", SELinux permissive"),
             and a kernel-install plugin that adds it for every new kernel

Nothing about the normal systemd boot changes: systemd, dbus-broker, journald
and all your units stay installed; the new entry is not made the default.
Backups of overwritten files go to $BACKUP.

EOF
}

next_steps() {
    cat <<EOF

Done. Next:
  1. Reboot and choose "$TITLE" in the GRUB menu.
  2. If stage 2 fails, log in on tty9 (Ctrl+Alt+F9) and look at:
       /run/uncaught-logs/current        (s6 catch-all log)
       s6-rc -a list                     (what is up)
  3. Once booted:  systemctl status, systemctl list-units --failed,
                   journalctl -b, journalctl -u systemd-logind, journalctl -u user@1000
  4. SELinux: the entry boots permissive. Collect denials with
       ausearch -m AVC -ts boot | audit2allow -M sdcompat_local
     review sdcompat_local.te, load it with semodule -i, then re-run with --enforcing.
  5. To go back: boot the normal Fedora entry. To remove everything:
       sudo $0 --uninstall
EOF
}

main() {
    check_system
    [ "$UNINSTALL" = 1 ] && uninstall
    print_plan
    confirm "Continue?"
    install_packages
    install_skarnet
    install_sdcompat
    configure_sdcompat
    enable_units
    build_database
    setup_s6_linux_init
    setup_selinux
    add_grub_entry
    next_steps
}

main
