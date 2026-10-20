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
#   --rpms=DIR      install the skarnet tools (dist/skarnet.versions) from RPMs
#                   built with dist/rpm/build-skarnet-rpms.sh (recommended:
#                   Fedora does not package them)
#   --build-s6      instead build them from source into /usr (untracked by rpm)
#   --rebuild       rebuild sdcompat and its tools even if they are built
#   --without=LIST  do not install these standalone tools (comma-separated):
#                   sdcompat-fdstore, sdcompat-notify, sdcompat-journal-relay
#                   (the last one is needed with --no-journal)
#   --remove-s6     with --uninstall: also remove the skarnet tools this script built
#   --no-journal    no systemd-journald: rsyslog + per-service s6-log directories
#                   (journalctl and the log lines in systemctl status won't work)
#   --journal       switch an install made with --no-journal back to journald
#   --no-resolved   no systemd-resolved on the s6 boot: NetworkManager handles
#                   DNS itself (the systemd boot keeps resolved)
#   --resolved      switch back to systemd-resolved on the s6 boot
#                   Without these four, a re-run keeps what the installed
#                   /etc/sdcompat/sdcompat.conf says.
set -euo pipefail

REPO=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
DIST=$REPO/dist
TITLE="Fedora (s6 + sdcompat)"
S6LI_BASE=/etc/s6-linux-init/current
LIBEXEC=/usr/libexec/sdcompat
SHARE=/usr/share/sdcompat
BACKUP=/var/lib/sdcompat/setup-backup/$(date +%Y%m%d-%H%M%S)

# The skarnet.org packages, their versions and the SHA-256 of their release
# tarballs, in build order. dist/skarnet.versions is the only place they are
# listed (build-skarnet-rpms.sh reads it too). s6-frontend is optional.
SKARNET=()
declare -A SKARNET_VER SKARNET_SHA256
while read -r name ver sum _; do
    case $name in ''|'#'*) continue ;; esac
    SKARNET+=("$name")
    SKARNET_VER[$name]=$ver
    SKARNET_SHA256[$name]=$sum
done <"$DIST/skarnet.versions"
SKARNET_OPTIONAL=" s6-frontend "
SKARNET_MANIFEST=/var/lib/sdcompat/skarnet-files.txt
# Checksums of the hand-written s6 services as this script last installed them.
S6_SUMS=/var/lib/sdcompat/s6-dist-sums.txt

RPM_DIR=""
# The core's tools (the Cargo workspace in $REPO) and the standalone ones
# (each its own project in $REPO/standalone).
CORE_TOOLS=(sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd)
STANDALONE=(sdcompat-fdstore sdcompat-notify sdcompat-journal-relay)
WITHOUT=""
DRY=0 YES=0 ENFORCING=0 GRUB=1 BUILD_S6=0 REBUILD=0 UNINSTALL=0 JOURNAL="" RESOLVED="" REMOVE_S6=0
KI_PLUGIN=/etc/kernel/install.d/90-sdcompat.install
for a in "$@"; do
    case $a in
        --dry-run) DRY=1 ;;
        --yes|-y) YES=1 ;;
        --enforcing) ENFORCING=1 ;;
        --no-grub) GRUB=0 ;;
        --build-s6) BUILD_S6=1 ;;
        --rpms=*) RPM_DIR=${a#--rpms=} ;;
        --rebuild) REBUILD=1 ;;
        --without=*) WITHOUT=,${a#--without=}, ;;
        --uninstall) UNINSTALL=1 ;;
        --no-journal) JOURNAL=0 ;;
        --journal) JOURNAL=1 ;;
        --no-resolved) RESOLVED=0 ;;
        --resolved) RESOLVED=1 ;;
        --remove-s6) REMOVE_S6=1 ;;
        -h|--help) sed -n '2,38p' "$0"; exit 0 ;;
        *) echo "unknown option: $a" >&2; exit 2 ;;
    esac
done

SDC_CONF=/etc/sdcompat/sdcompat.conf
# conf_bool KEY: 0 if the installed config sets KEY to a false value, else 1.
conf_bool() {
    local v
    v=$(sed -n "s/^[[:space:]]*$1=[[:space:]]*//p" "$SDC_CONF" 2>/dev/null | tail -n 1)
    case ${v,,} in no|false|0|off|n|f) echo 0 ;; *) echo 1 ;; esac
}
JOURNAL=${JOURNAL:-$(conf_bool JOURNAL)}
RESOLVED=${RESOLVED:-$(conf_bool RESOLVED)}
# wanted NAME: whether the standalone tool NAME is to be installed.
wanted() { case $WITHOUT in *",$1,"*) return 1 ;; *) return 0 ;; esac; }
if [ "$JOURNAL" = 0 ] && ! wanted sdcompat-journal-relay; then
    echo "error: --no-journal needs sdcompat-journal-relay (drop it from --without)" >&2
    exit 2
fi

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

# Copy a directory tree from the source checkout: root-owned (not cp -a, which
# keeps the checkout owner's uid on files root runs, and its home labels).
copy_tree() {
    run cp -dR --preserve=mode,timestamps,links "$1" "$2"
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
    if grep -q "written by sdcompat's" /etc/s6.conf 2>/dev/null; then
        run rm -f /etc/s6.conf
    fi
    run rm -rf "$S6LI_BASE"
    local t
    for t in "${STANDALONE[@]}"; do
        if command -v rpm >/dev/null && rpm -q "$t" >/dev/null 2>&1; then
            echo "  $t is installed as an RPM; remove it with: dnf remove $t"
        else
            run rm -f "$LIBEXEC/$t"
        fi
    done
    if command -v rpm >/dev/null && rpm -q sdcompat >/dev/null 2>&1; then
        echo "  sdcompat is installed as an RPM; remove it with: dnf remove sdcompat"
    else
        run rm -rf "$LIBEXEC" "$SHARE" /usr/bin/sdcompat /usr/sbin/init-selinux \
            /usr/share/dbus-1/system.d/sdcompat.conf
        if command -v semodule >/dev/null && semodule -l 2>/dev/null | grep -qx sdcompat; then
            run semodule -r sdcompat
        fi
    fi
    if [ "$REMOVE_S6" = 1 ] && [ -r "$SKARNET_MANIFEST" ]; then
        echo "  removing the skarnet tools listed in $SKARNET_MANIFEST"
        while read -r f; do
            case $f in /usr/*) run rm -f "$f" ;; esac
        done <"$SKARNET_MANIFEST"
        run rm -f "$SKARNET_MANIFEST"
    fi
    if command -v restorecon >/dev/null; then
        local files
        mapfile -t files < <(skarnet_files)
        [ "${#files[@]}" -gt 0 ] && run restorecon -R "${files[@]}"
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

# LUKS: volumes the initramfs unlocks (rd.luks.uuid=) are open before s6
# starts. Anything else in /etc/crypttab is opened by systemd-cryptsetup@
# units on a systemd boot, which do not exist under s6.
check_luks() {
    [ -s /etc/crypttab ] || return 0
    say "Checking encrypted volumes (/etc/crypttab)"
    local cmdline name dev uuid late=()
    cmdline=$(cat /proc/cmdline)
    while read -r name dev _; do
        case $name in ''|\#*) continue ;; esac
        uuid=${dev#UUID=}
        if [[ " $cmdline " == *" rd.luks.uuid=$uuid "* || " $cmdline " == *" rd.luks.uuid=luks-$uuid "* \
              || " $cmdline " == *" rd.luks.name=$uuid="* ]]; then
            echo "  $name: unlocked by the initramfs (fine)"
        else
            late+=("$name")
        fi
    done </etc/crypttab
    if [ ${#late[@]} -gt 0 ]; then
        warn "not unlocked by the initramfs: ${late[*]}"
        warn "  Under s6 nothing opens these (systemd-cryptsetup@ units are systemd-only),"
        warn "  so filesystems on them will not be mounted. Add them to the initramfs with"
        warn "  rd.luks.uuid= on the kernel command line, or leave them unused for this test."
        confirm "Continue anyway?"
    fi
    echo "  The LUKS passphrase prompt will be plain text (the s6 entry disables plymouth)."
}

install_packages() {
    say "Installing Fedora packages"
    local pkgs=(dbus-daemon earlyoom chrony NetworkManager
                policycoreutils-python-utils grubby curl tar gcc make patch)
    [ "$JOURNAL" = 1 ] || pkgs+=(rsyslog)
    [ -x "$REPO/target/release/sdcompatd" ] && [ "$REBUILD" = 0 ] || pkgs+=(cargo rust)
    if ! rpm -q gdm gnome-shell >/dev/null 2>&1; then
        warn "gdm/gnome-shell are not installed. For a full desktop run:"
        warn "  dnf group install workstation-product-environment"
    fi
    run dnf install -y "${pkgs[@]}"
}

install_skarnet() {
    say "Installing ${SKARNET[*]}"
    local missing=() p
    if [ -n "$RPM_DIR" ]; then
        local rpms=()
        local f
        for f in "$RPM_DIR"/*.rpm; do
            case $f in *.src.rpm|*-debuginfo-*|*-debugsource-*) ;; *) [ -e "$f" ] && rpms+=("$f") ;; esac
        done
        [ ${#rpms[@]} -gt 0 ] || [ "$DRY" = 1 ] || die "no RPMs in $RPM_DIR (build them with dist/rpm/build-skarnet-rpms.sh)"
        run dnf install -y "${rpms[@]}"
        return
    fi
    if [ "$BUILD_S6" = 0 ]; then
        local want=()
        for p in "${SKARNET[@]}"; do
            if rpm -q "$p" >/dev/null 2>&1 || dnf -q list --available "$p" >/dev/null 2>&1; then
                want+=("$p")
            elif [[ $SKARNET_OPTIONAL != *" $p "* ]]; then
                missing+=("$p")
            fi
        done
        if [ ${#missing[@]} = 0 ]; then
            run dnf install -y "${want[@]}"
            return
        fi
        if [ "$DRY" = 0 ]; then
            die "not packaged for this Fedora release: ${missing[*]}.
  Recommended: build RPMs with   dist/rpm/build-skarnet-rpms.sh
               then run          sudo $0 --rpms=\$HOME/rpmbuild-skarnet/rpms
  Or install from source into /usr (not tracked by rpm) with --build-s6."
        fi
        warn "not packaged: ${missing[*]} (dry run continues as if --build-s6 were given)"
    fi
    # Building into /usr must not overwrite files owned by installed RPMs, and
    # they must be versions that fit together.
    local owned=()
    for p in "${SKARNET[@]}"; do
        rpm -q "$p" >/dev/null 2>&1 && owned+=("$p")
    done
    if [ ${#owned[@]} -gt 0 ]; then
        die "already installed as RPMs: ${owned[*]}. Remove them (dnf remove ${owned[*]}) so all are built as one matching set, or package the missing ones."
    fi
    # Build in dependency order into /usr (statically linked against skalibs).
    local work=/tmp/sdcompat-skarnet-build
    [ "$DRY" = 1 ] || work=$(mktemp -d)
    local name
    for name in "${SKARNET[@]}"; do
        local ver=${SKARNET_VER[$name]} sum=${SKARNET_SHA256[$name]}
        echo "  building $name $ver"
        run curl -fsSL -o "$work/$name.tar.gz" "https://skarnet.org/software/$name/$name-$ver.tar.gz"
        if [ "$DRY" = 0 ]; then
            echo "$sum  $work/$name.tar.gz" | sha256sum -c --quiet - || die "checksum mismatch for $name-$ver"
        else
            printf '  + verify sha256 %s\n' "$sum"
        fi
        run tar -xzf "$work/$name.tar.gz" -C "$work"
        for p in "$DIST/rpm/$name-$ver"-*.patch; do
            if [ -e "$p" ]; then run patch -d "$work/$name-$ver" -p1 -i "$p"; fi
        done
        # As in the RPM specs: execline as one multicall binary (the SELinux
        # module labels /usr/bin/execline); s6-frontend reads /etc/s6.conf
        # (not PREFIX/etc) and aligns its tables with util-linux's column.
        local conf=(--prefix=/usr)
        case $name in
            execline) conf+=(--enable-multicall) ;;
            s6-frontend) conf+=(--sysconfdir=/etc --enable-util-linux) ;;
        esac
        local stage=$work/stage-$name
        if [ "$DRY" = 0 ]; then
            # Install into a stage, record it (for --uninstall --remove-s6),
            # then copy it to /: later packages build against it.
            (cd "$work/$name-$ver" && ./configure "${conf[@]}" >/dev/null && make -j"$(nproc)" >/dev/null \
                && make install DESTDIR="$stage" >/dev/null)
            if [ "$name" = execline ]; then
                # bash owns /usr/bin/cd, umask and wait: execline's go out of PATH.
                mkdir -p "$stage/usr/libexec/execline"
                for p in cd umask wait; do
                    rm -f "$stage/usr/bin/$p"
                    ln -s /usr/bin/execline "$stage/usr/libexec/execline/$p"
                done
            fi
            (cd "$stage" && find . \( -type f -o -type l \) | sed 's|^\.||') >>"$work/manifest"
            # Not cp -a: that would also copy the /tmp SELinux labels.
            cp -dR --preserve=mode,timestamps,links "$stage/." /
        else
            printf '  + (cd %s && ./configure %s && make && make install DESTDIR=%s) && cp -dR %s/. /\n' \
                "$work/$name-$ver" "${conf[*]}" "$stage" "$stage"
        fi
    done
    if [ "$DRY" = 0 ]; then
        mkdir -p "$(dirname "$SKARNET_MANIFEST")"
        sort -u "$work/manifest" >"$SKARNET_MANIFEST"
        rm -rf "$work"
    fi
}

# build DIR: cargo build --release in DIR, as the user who ran sudo.
build() {
    local builder=${SUDO_USER:-root}
    if [ "$builder" != root ]; then
        run su "$builder" -c "cd '$1' && cargo build --release"
    else
        run sh -c "cd '$1' && cargo build --release"
    fi
}

# The standalone tools: each is its own project and its own RPM.
install_standalone() {
    local t
    for t in "${STANDALONE[@]}"; do
        wanted "$t" || continue
        if rpm -q "$t" >/dev/null 2>&1; then
            echo "  $t RPM is installed, using it"
            continue
        fi
        local dir=$REPO/standalone/$t
        [ -x "$dir/target/release/$t" ] && [ "$REBUILD" = 0 ] || build "$dir"
        run install -Dm0755 "$dir/target/release/$t" "$LIBEXEC/$t"
    done
}

install_sdcompat() {
    say "Installing sdcompat"
    install_standalone
    if rpm -q sdcompat >/dev/null 2>&1; then
        echo "  sdcompat RPM is installed, using it"
        return
    fi
    if [ ! -x "$REPO/target/release/sdcompatd" ] || [ "$REBUILD" = 1 ]; then
        build "$REPO"
    fi
    local t
    for t in "${CORE_TOOLS[@]}"; do
        run install -Dm0755 "$REPO/target/release/$t" "$LIBEXEC/$t"
    done
    run install -Dm0755 "$DIST/early" "$LIBEXEC/early"
    run ln -sfn ../libexec/sdcompat/sdcompat /usr/bin/sdcompat
    run install -Dm0644 "$DIST/dbus/sdcompat.conf" /usr/share/dbus-1/system.d/sdcompat.conf
    run install -Dm0755 "$DIST/s6-linux-init/init-selinux" /usr/sbin/init-selinux
    run mkdir -p "$SHARE/s6-linux-init"
    run rm -rf "$SHARE/s6-linux-init/skel"
    copy_tree "$DIST/s6-linux-init/skel" "$SHARE/s6-linux-init/skel"
    run install -Dm0644 "$DIST/rsyslog.conf" "$SHARE/rsyslog.conf"
    run install -Dm0644 "$DIST/selinux/sdcompat.cil" "$SHARE/selinux/sdcompat.cil"
}

# set_conf_key KEY yes|no: "no" sets KEY=no in sdcompat.conf, "yes" removes
# the setting (both default to yes). Other lines are kept.
set_conf_key() {
    local key=$1 val=$2 flag text
    flag="--no-$(echo "$key" | tr '[:upper:]' '[:lower:]')"
    [ "$(conf_bool "$key")" = "$([ "$val" = no ] && echo 0 || echo 1)" ] && return 0
    echo "  $key=$val in $SDC_CONF"
    # Read it all before put truncates the file ($() also drops trailing blank lines).
    text=$(grep -v -e "^[[:space:]]*$key=" -e "^# set by setup-fedora.sh $flag\$" "$SDC_CONF")
    [ "$val" = no ] && text+=$'\n\n'"# set by setup-fedora.sh $flag"$'\n'"$key=no"
    printf '%s\n' "$text" | put "$SDC_CONF"
}

configure_sdcompat() {
    say "Configuring /etc/sdcompat"
    if [ -e "$SDC_CONF" ]; then
        echo "  keeping existing $SDC_CONF"
    else
        put "$SDC_CONF" <"$DIST/sdcompat.conf"
    fi
    if [ "$DRY" = 1 ] && [ ! -e "$SDC_CONF" ]; then
        [ "$JOURNAL" = 1 ] || printf '  + JOURNAL=no in %s\n' "$SDC_CONF"
        [ "$RESOLVED" = 1 ] || printf '  + RESOLVED=no in %s\n' "$SDC_CONF"
    else
        set_conf_key JOURNAL "$([ "$JOURNAL" = 1 ] && echo yes || echo no)"
        set_conf_key RESOLVED "$([ "$RESOLVED" = 1 ] && echo yes || echo no)"
    fi
    [ "$JOURNAL" = 1 ] || put /etc/sdcompat/rsyslog.conf <"$DIST/rsyslog.conf"
    # The s6 command: sdcompat's database instead of s6-rc's default bootdb.
    if [ -e /etc/s6.conf ]; then
        echo "  keeping existing /etc/s6.conf"
    else
        put /etc/s6.conf <"$DIST/s6.conf"
    fi

    install_s6_services
    # With journald, it owns /dev/log; the s6-only rsyslog is not started.
    # Without it, journal-relay serves journald's client sockets instead.
    sysinit_member syslog "$([ "$JOURNAL" = 1 ] && echo 0 || echo 1)"
    sysinit_member journal-relay "$([ "$JOURNAL" = 1 ] && echo 0 || echo 1)"
    # Without resolved, "resolv" hands DNS to NetworkManager at boot.
    sysinit_member resolv "$([ "$RESOLVED" = 1 ] && echo 0 || echo 1)"
    run mkdir -p /var/lib/sdcompat /var/log/sdcompat /var/lib/rsyslog
}

# sysinit_member NAME 0|1: take the s6 service NAME out of, or put it into,
# the sdcompat-sysinit bundle.
sysinit_member() {
    local f=/etc/sdcompat/s6/sdcompat-sysinit/contents.d/$1
    if [ "$2" = 0 ]; then
        [ -e "$f" ] && run rm -f "$f"
    elif [ ! -e "$f" ]; then
        run touch "$f"
    fi
    return 0
}

# Checksum of a hand-written s6 service directory: names, types, link targets,
# the executable bit and file contents (not other mode bits, which depend on
# the umask of the checkout). contents.d/syslog, contents.d/journal-relay and
# contents.d/resolv are left out: this script adds or removes them itself.
s6_dir_sum() {
    local skip=(! -path ./contents.d/syslog ! -path ./contents.d/journal-relay ! -path ./contents.d/resolv)
    (cd "$1" && {
        find . -mindepth 1 "${skip[@]}" -printf '%y %p %l\n'
        find . -type f -perm -u+x "${skip[@]}" -printf 'x %p\n'
        find . -type f "${skip[@]}" -print0 | xargs -0r sha256sum
    } | LC_ALL=C sort) | sha256sum | cut -d' ' -f1
}

# Whether checksum $2 is a version of dist/s6/$1 from this repo's git history.
# Installs made before $S6_SUMS existed have no record, and this tells an old
# shipped version from the owner's edits.
s6_past_version() {
    local name=$1 sum=$2 c tmp found=1
    [ -d "$REPO/.git" ] && command -v git >/dev/null || return 1
    tmp=$(mktemp -d)
    for c in $(git -c safe.directory="$REPO" -C "$REPO" rev-list HEAD -- "dist/s6/$name"); do
        rm -rf "${tmp:?}"/*
        git -c safe.directory="$REPO" -C "$REPO" archive "$c" "dist/s6/$name" | tar -x -C "$tmp" || continue
        if [ "$(s6_dir_sum "$tmp/dist/s6/$name")" = "$sum" ]; then
            found=0
            break
        fi
    done
    rm -rf "$tmp"
    return $found
}

# Hand-written s6 services: add missing ones and update the ones still as an
# earlier setup installed them; keep the ones you edited.
install_s6_services() {
    run mkdir -p /etc/sdcompat/s6
    if rpm -q sdcompat >/dev/null 2>&1; then
        echo "  sdcompat RPM is installed, it owns /etc/sdcompat/s6"
        return
    fi
    local -A recorded=()
    local d name dest new cur sums=""
    if [ -r "$S6_SUMS" ]; then
        while read -r name cur; do recorded[$name]=$cur; done <"$S6_SUMS"
    fi
    for d in "$DIST"/s6/*/; do
        name=$(basename "$d")
        dest=/etc/sdcompat/s6/$name
        new=$(s6_dir_sum "$d")
        if [ ! -e "$dest" ]; then
            copy_tree "$d" "$dest"
        else
            cur=$(s6_dir_sum "$dest")
            if [ "$cur" = "$new" ]; then
                :
            elif [ "$cur" = "${recorded[$name]:-}" ] || s6_past_version "$name" "$cur"; then
                echo "  updating $dest (not edited since an earlier setup)"
                backup "$dest"
                run rm -rf "$dest"
                copy_tree "$d" "$dest"
            else
                echo "  keeping your modified $dest (new version: $d)"
                new=${recorded[$name]:-}
            fi
        fi
        [ -n "$new" ] && sums+="$name $new"$'\n'
    done
    # Earlier versions copied with cp -a and left them owned by the checkout's owner.
    run chown -R root:root /etc/sdcompat/s6
    if [ "$DRY" = 1 ]; then
        printf '  + write %s\n' "$S6_SUMS"
    else
        mkdir -p "$(dirname "$S6_SUMS")"
        printf '%s' "$sums" >"$S6_SUMS"
    fi
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
        printf '  + sdcompat-gen reload --no-update\n'
        return
    fi
    "$LIBEXEC/sdcompat-gen" reload --no-update
    local report=/var/lib/sdcompat/current/report.txt
    echo "  report: $report"
    grep -E '^(translated|skipped|cycles broken):' "$report" | sed 's/^/  /'
    local n
    n=$(grep -c 'warning:' "$report" || true)
    echo "  $n warnings (mostly sandboxing directives that are not enforced)"
}

# When run on the s6 boot, restart sdcompatd so the binary just installed
# takes over without a reboot. The daemon keeps dynamic units, the fd store
# and device state across a restart. sdcompat-fdstore and journal-relay are
# left alone: restarting them would drop held fds and log sockets, so they
# keep the old binary until the next boot.
restart_daemon() {
    local svc=/run/s6-rc/servicedirs/sdcompatd
    [ -d "$svc" ] || return 0
    [ "$DRY" = 1 ] || s6-svok "$svc" 2>/dev/null || return 0
    say "Restarting sdcompatd (running on the s6 boot)"
    if ! run s6-svc -wR -T 30000 -r "$svc"; then
        warn "sdcompatd was not ready within 30 s; check s6-svstat $svc"
    fi
}

setup_s6_linux_init() {
    say "Generating s6-linux-init in $S6LI_BASE"
    local tmp=/tmp/sdcompat-s6li
    [ "$DRY" = 1 ] || tmp=$(mktemp -d)
    # -n: keep the initramfs's /run (remount it) as systemd does. Without
    # it s6-linux-init's umount fails (credential mounts under it) and a
    # second tmpfs hides the udev database, /run/log/journal, utab, utmp.
    run s6-linux-init-maker \
        -n \
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
    # ... and the whole s6 tree must stay there, so that services enter their
    # own domains as under systemd (see dist/selinux/sdcompat.cil).
    run semodule -i "$SHARE/selinux/sdcompat.cil"
    local files
    mapfile -t files < <(skarnet_files)
    run restorecon -R "$S6LI_BASE" "$LIBEXEC" /usr/sbin/init-selinux /etc/sdcompat "$SHARE" \
        /var/lib/sdcompat /var/log/sdcompat /usr/share/dbus-1/system.d/sdcompat.conf "${files[@]}"
    [ -d /run/sdcompat ] && run restorecon -R /run/sdcompat
    # On a running s6 boot this lets a stuck catch-all logger rotate again.
    [ -d /run/uncaught-logs ] && run restorecon -R /run/uncaught-logs
    return 0
}

# The installed programs that dist/selinux/sdcompat.cil labels init_exec_t.
skarnet_files() {
    local f
    for f in /usr/bin/s6-* /usr/bin/ucspilogd /usr/bin/execline; do
        [ -e "$f" ] && echo "$f"
    done
    for f in /usr/libexec/execline /usr/libexec/s6 /usr/libexec/s6-rc; do
        [ -d "$f" ] && echo "$f"
    done
    return 0
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
    ($( [ -n "$RPM_DIR" ] && echo "RPMs from $RPM_DIR" || { [ "$BUILD_S6" = 1 ] && echo "built from skarnet.org into /usr" || echo "Fedora packages"; } )), sdcompat
  * write:   /etc/sdcompat/ (config, s6 services, boot options),
             $S6LI_BASE, /usr/sbin/init-selinux, $LIBEXEC
  * enable:  chronyd, NetworkManager, gdm; default target graphical.target
  * SELinux: label $S6LI_BASE/bin/init as init_exec_t; load the policy module
             $SHARE/selinux/sdcompat.cil (s6 tools and sdcompat as init_exec_t,
             so services run in their own domains as under systemd)
  * logs:    $([ "$JOURNAL" = 1 ] && echo "systemd-journald as an s6 service (journalctl works)" || echo "rsyslog + s6-log directories, no journal")
  * DNS:     $([ "$RESOLVED" = 1 ] && echo "systemd-resolved as on the systemd boot" || echo "NetworkManager without systemd-resolved (s6 boot only)")
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
  5. If a new s6 database breaks booting, add one of these to the kernel
     command line in GRUB (press e):
       sdcompat.db=previous    boot the database from before the last change
       sdcompat.reload=0       do not regenerate the database at boot
  6. To go back: boot the normal Fedora entry. To remove everything:
       sudo $0 --uninstall [--remove-s6]
EOF
}

main() {
    check_system
    [ "$UNINSTALL" = 1 ] && uninstall
    print_plan
    check_luks
    confirm "Continue?"
    install_packages
    install_skarnet
    install_sdcompat
    configure_sdcompat
    enable_units
    build_database
    restart_daemon
    setup_s6_linux_init
    setup_selinux
    add_grub_entry
    next_steps
}

main
