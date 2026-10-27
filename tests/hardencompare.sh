#!/bin/bash
# Compare how each service is confined on the systemd boot and on the s6
# boot: the settings systemd's sandboxing directives leave behind in /proc,
# read from every running service's main process.
#
#   sudo tests/hardencompare.sh snap [DIR]
#   tests/hardencompare.sh compare SYSTEMD.tsv S6.tsv
#   tests/hardencompare.sh report [DIR]
#
# snap writes DIR/harden-INIT-TIME.tsv (DIR: ~/uinitconverter-mem/harden of
# the user who ran sudo). Take one on each boot, after the services you care
# about have started. compare lists, per service running on both, what
# differs: WEAKER where the s6 boot confines it less (no_new_privs off,
# less seccomp, more capabilities, a host namespace instead of a private
# one, a read-write mount that was read-only, root instead of a user, an
# unconfined SELinux domain, fewer cgroup BPF programs), DIFF otherwise.
# report compares the latest snapshot of each init in DIR. Exit status:
# 0 nothing weaker, 1 something weaker, 2 a snapshot missing.
#
# /proc doesn't show which syscalls a seccomp filter allows, so
# SystemCallFilter=, RestrictAddressFamilies=, MemoryDenyWriteExecute= and
# the like show only as the number of filters.
set -u

# HARDEN_* overrides are for tests/hardencompare-check.sh.
CGROOT=${HARDEN_CGROUP_ROOT:-/sys/fs/cgroup}
PATHS="/ /usr /etc /boot /efi /var /home /root /run /run/user /tmp /var/tmp /dev /dev/shm /proc /proc/sys /sys /sys/fs/cgroup /usr/lib/modules"
NS="mnt net uts ipc pid user cgroup"
CAPS="chown dac_override dac_read_search fowner fsetid kill setgid setuid setpcap linux_immutable net_bind_service net_broadcast net_admin net_raw ipc_lock ipc_owner sys_module sys_rawio sys_chroot sys_ptrace sys_pacct sys_admin sys_boot sys_nice sys_resource sys_time sys_tty_config mknod lease audit_write audit_control setfcap mac_override mac_admin syslog wake_alarm block_suspend audit_read perfmon bpf checkpoint_restore"

usage() {
    sed -n '2,23s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

die() {
    echo "hardencompare: $*" >&2
    exit 2
}

current_init() {
    if [ -n "${HARDEN_INIT:-}" ]; then
        echo "$HARDEN_INIT"
        return
    fi
    case $(cat /proc/1/comm) in
        s6-svscan) echo s6 ;;
        systemd) echo systemd ;;
        *) cat /proc/1/comm ;;
    esac
}

default_dir() {
    echo "$(getent passwd "${SUDO_USER:-$(id -un)}" | cut -d: -f6)/uinitconverter-mem/harden"
}

# Capability names in a hex mask, comma-separated; "none" for 0.
cap_names() {
    local mask=$((16#$1)) i=0 out="" c
    for c in $CAPS; do
        if (((mask >> i) & 1)); then out+=${out:+,}$c; fi
        i=$((i + 1))
    done
    # Bits above checkpoint_restore, by number.
    while [ $i -lt 64 ]; do
        if (((mask >> i) & 1)); then out+=${out:+,}cap$i; fi
        i=$((i + 1))
    done
    echo "${out:-none}"
}

# The service's main process: the oldest one in its cgroup that isn't
# uinitconverter's or s6's (under s6 the runner exec()s into the service, so
# the main process has the runner's PID).
main_pid() {
    local best="" bt=0 p st t
    while read -r p; do
        [ -r "/proc/$p/stat" ] || continue
        case $(cat "/proc/$p/comm" 2>/dev/null) in uinitconverter* | s6-*) continue ;; esac
        st=$(cat "/proc/$p/stat" 2>/dev/null) || continue
        # Field 22 (starttime), counted after the ")" that ends comm.
        t=$(echo "${st##*) }" | cut -d' ' -f20)
        if [ -z "$best" ] || [ "$t" -lt "$bt" ]; then best=$p bt=$t; fi
    done <"$1/cgroup.procs"
    echo "$best"
}

# One "unit NAME FIELD VALUE" row per setting of PID.
fields() {
    local u=$1 p=$2 cg=$3 k v n
    row() { printf 'unit\t%s\t%s\t%s\n' "$u" "$1" "$2"; }
    row pid "$p"
    row comm "$(cat "/proc/$p/comm")"
    row exe "$(readlink "/proc/$p/exe" 2>/dev/null || echo -)"
    while IFS=$'\t' read -r k v; do
        case $k in
            Uid:) row uid "$(echo "$v" | cut -f2)" ;;
            Gid:) row gid "$(echo "$v" | cut -f2)" ;;
            Groups:) v=$(echo "$v" | xargs); row groups "${v:-none}" ;;
            Umask:) row umask "$v" ;;
            NoNewPrivs:) row nnp "$v" ;;
            Seccomp:) row seccomp "$v" ;;
            Seccomp_filters:) row seccomp_filters "$v" ;;
            CapInh:) row capinh "$(cap_names "$v")" ;;
            CapPrm:) row capprm "$(cap_names "$v")" ;;
            CapEff:) row capeff "$(cap_names "$v")" ;;
            CapBnd:) row capbnd "$(cap_names "$v")" ;;
            CapAmb:) row capamb "$(cap_names "$v")" ;;
        esac
    done <"/proc/$p/status"
    v=$(tr -d '\0' <"/proc/$p/attr/current" 2>/dev/null)
    row selinux "${v:--}"
    for n in $NS; do
        k=$(readlink "/proc/$p/ns/$n" 2>/dev/null) || { row "ns_$n" -; continue; }
        if [ "$k" = "$(readlink "/proc/1/ns/$n" 2>/dev/null)" ]; then row "ns_$n" host; else row "ns_$n" private; fi
    done
    # The mount each path lies on: mount point, type, ro/rw, and "bind" when
    # it shows a subdirectory of its file system (PrivateTmp=, bind mounts).
    # A per-boot directory name (systemd-private-*) is left out.
    awk -v paths="$PATHS" -v u="$u" '
    {
        for (i = 7; $i != "-"; i++) {}
        mp[NR] = $5; desc[NR] = $(i + 1) " " ($6 ~ /(^|,)ro(,|$)/ ? "ro" : "rw") ($4 == "/" ? "" : " bind")
    }
    END {
        n = split(paths, P, " ")
        for (j = 1; j <= n; j++) {
            best = 0
            for (m = 1; m <= NR; m++) {
                if (P[j] == mp[m] || mp[m] == "/" || index(P[j], mp[m] "/") == 1)
                    if (!best || length(mp[m]) >= length(mp[best])) best = m
            }
            printf "unit\t%s\tmount:%s\t%s\n", u, P[j], best ? mp[best] " " desc[best] : "-"
        }
    }' "/proc/$p/mountinfo" 2>/dev/null
    awk -v u="$u" '
    function lim(name, f) { printf "unit\t%s\tlimit_%s\t%s/%s\n", u, name, $(f), $(f + 1) }
    /^Max open files/ { lim("nofile", 4) }
    /^Max processes/ { lim("nproc", 3) }
    /^Max locked memory/ { lim("memlock", 4) }
    /^Max core file size/ { lim("core", 5) }
    ' "/proc/$p/limits" 2>/dev/null
    row oom_score_adj "$(cat "/proc/$p/oom_score_adj" 2>/dev/null || echo -)"
    if command -v bpftool >/dev/null; then
        row bpf_progs "$(bpftool cgroup show "$cg" 2>/dev/null | tail -n +2 | grep -c .)"
    fi
}

snap() {
    local dir=${1:-$(default_dir)} init out cg u p
    [ "$(id -u)" = 0 ] || [ -n "${HARDEN_TEST:-}" ] || die "snap needs root (sudo)"
    init=$(current_init)
    mkdir -p "$dir" || exit 2
    out=$dir/harden-$init-$(date +%Y%m%d-%H%M%S).tsv
    {
        printf 'meta\tinit\t%s\nmeta\tkernel\t%s\nmeta\ttime\t%s\n' "$init" "$(uname -r)" "$(date '+%F %T')"
        # Every service cgroup, but not the ones nested inside a service
        # (user@UID.service's own).
        find "$CGROOT/system.slice" -type d -name '*.service' -prune -print 2>/dev/null | sort | while read -r cg; do
            u=${cg##*/}
            p=$(main_pid "$cg")
            [ -n "$p" ] || continue
            fields "$u" "$p" "$cg"
        done
    } >"$out.tmp" && mv "$out.tmp" "$out" || exit 2
    [ -n "${SUDO_USER:-}" ] && chown -R "$SUDO_USER:" "$dir" 2>/dev/null
    echo "$out: $(awk -F'\t' '$1 == "unit" && $3 == "pid"' "$out" | wc -l) services ($init)"
}

compare() {
    [ $# = 2 ] || usage
    local f
    for f in "$1" "$2"; do [ -r "$f" ] || die "$f: not readable"; done
    awk -F'\t' '
    FNR == 1 { side++ }
    $1 == "meta" { meta[side, $2] = $3 }
    $1 == "unit" {
        val[side, $2, $3] = $4; units[$2] = 1; has[side, $2] = 1
        if (!($3 in seen)) { seen[$3] = 1; order[++nf] = $3 }
    }
    # DynamicUser= ids are picked per boot.
    function norm(v) { return (v ~ /^[0-9]+$/ && v + 0 >= 61184 && v + 0 <= 65519) ? "dynamic" : v }
    function setdiff(a, b,   A, B, n, i, out) {
        n = split(a, A, ","); split(b, B, ",")
        for (i in B) inb[B[i]] = 1
        out = ""
        for (i = 1; i <= n; i++) if (A[i] != "none" && !(A[i] in inb)) out = out (out ? "," : "") A[i]
        for (i in inb) delete inb[i]
        return out
    }
    # A read-only mount turned read-write, a mount systemd put on the path
    # (ProtectHome=, PrivateTmp=, InaccessiblePaths=) missing, or a private
    # bind mount replaced by the host directory.
    function mount_weaker(a, b,   A, B) {
        split(a, A, " "); split(b, B, " ")
        if (A[3] == "ro" && B[3] == "rw") return 1
        if (A[1] != B[1]) return length(A[1]) > length(B[1])
        return A[4] == "bind" && B[4] != "bind"
    }
    function seltype(c,   s) { split(c, s, ":"); return s[3] }
    # "W" when the s6 value (b) confines less than systemd (a), else "D".
    function judge(f, a, b,   x) {
        if (f == "nnp") return (a == 1 && b != 1) ? "W" : "D"
        if (f == "seccomp") return (b + 0 < a + 0) ? "W" : "D"
        if (f ~ /^cap/) { x = setdiff(b, a); return x != "" ? "W" : "D" }
        if (f ~ /^ns_/) return (a == "private" && b == "host") ? "W" : "D"
        if (f ~ /^mount:/) return mount_weaker(a, b) ? "W" : "D"
        if (f == "uid") return (a != "0" && b == "0") ? "W" : "D"
        if (f == "selinux") return (seltype(b) ~ /^(unconfined_service_t|unconfined_t|init_t|initrc_t)$/ && seltype(a) != seltype(b)) ? "W" : "D"
        if (f == "bpf_progs") return (a + 0 > 0 && b + 0 == 0) ? "W" : "D"
        return "D"
    }
    END {
        printf "Hardening comparison: systemd %s (%s), s6 %s (%s)\n", meta[1, "time"], meta[1, "kernel"], meta[2, "time"], meta[2, "kernel"]
        if (meta[1, "init"] != "systemd" || meta[2, "init"] != "s6")
            printf "warning: the first snapshot is from %s, the second from %s\n", meta[1, "init"], meta[2, "init"]
        n = 0
        for (u in units) list[++n] = u
        # Sorted by name (insertion sort: no asort outside gawk).
        for (i = 2; i <= n; i++) { x = list[i]; for (j = i - 1; j > 0 && list[j] > x; j--) list[j + 1] = list[j]; list[j + 1] = x }
        weak = 0; diff = 0; both = 0; o1 = ""; o2 = ""
        for (i = 1; i <= n; i++) {
            u = list[i]
            if (!((1, u) in has)) { o2 = o2 " " u; continue }
            if (!((2, u) in has)) { o1 = o1 " " u; continue }
            both++
            lines = ""; w = 0; d = 0
            for (k = 1; k <= nf; k++) {
                f = order[k]
                if (f == "pid") continue
                a = val[1, u, f]; b = val[2, u, f]
                if (f == "uid" || f == "gid") { a = norm(a); b = norm(b) }
                if (a == b) continue
                j = judge(f, a, b)
                extra = ""
                if (f ~ /^cap/) {
                    x = setdiff(b, a); y = setdiff(a, b)
                    extra = (x != "" ? " (s6 adds " x ")" : "") (y != "" ? " (s6 drops " y ")" : "")
                    a = length(a) > 40 ? "..." : a; b = length(b) > 40 ? "..." : b
                }
                if (j == "W") w++; else d++
                lines = lines sprintf("  %s %s: systemd=%s s6=%s%s\n", j == "W" ? "WEAKER" : "DIFF  ", f, a == "" ? "(none)" : a, b == "" ? "(none)" : b, extra)
            }
            weak += w; diff += d
            if (lines != "") printf "\n%s: %d weaker, %d different\n%s", u, w, d, lines
        }
        printf "\nServices on both boots: %d; settings weaker on s6: %d, different: %d\n", both, weak, diff
        if (o1 != "") printf "Running only on systemd:%s\n", o1
        if (o2 != "") printf "Running only on s6:%s\n", o2
        printf "Result: %s\n", weak ? "WEAKER on s6 (see above)" : "nothing weaker on s6"
        exit weak ? 1 : 0
    }' "$1" "$2"
}

report() {
    local dir=${1:-$(default_dir)} s y
    y=$(compgen -G "$dir/harden-systemd-*.tsv" | sort | tail -n 1)
    s=$(compgen -G "$dir/harden-s6-*.tsv" | sort | tail -n 1)
    if [ -z "$y" ] || [ -z "$s" ]; then
        echo "Result: INCOMPLETE: needs a snapshot of each init in $dir (systemd: ${y:-none}, s6: ${s:-none})"
        exit 2
    fi
    echo "Snapshots: ${y##*/} ${s##*/}"
    compare "$y" "$s"
}

case ${1:-} in
    snap) shift; [ $# -le 1 ] || usage; snap "$@" ;;
    compare) shift; compare "$@" ;;
    report) shift; [ $# -le 1 ] || usage; report "$@" ;;
    *) usage ;;
esac
