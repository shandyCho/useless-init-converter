#!/bin/bash
# Compare memory use between the s6 boot and the systemd boot.
#
#   sudo tests/memcompare.sh snap [--wait SECS] [--exclude-cgroup UNIT] [DIR]
#                                                       # once on each boot
#   tests/memcompare.sh compare FILE_A FILE_B           # afterwards, on either
#
# snap waits until the system has been up SECS seconds (default 300) so both
# boots are measured after the same settling time, then writes
# DIR/mem-<init>-<date>.tsv (DIR defaults to ~/uinitconverter-mem of the user who
# ran sudo). Without root it still works, but other users' processes have no
# PSS and the comparison falls back to private anonymous memory (RssAnon).
# --exclude-cgroup leaves out the processes of a unit's cgroup (the
# measuring service of tests/memtest.sh). It only reads; nothing is changed.
set -u

usage() {
    sed -n '2,15s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

kb() { # first number of "  1234 kB"
    local v=${1#"${1%%[! ]*}"}
    echo "${v%% *}"
}

# Which group a process belongs to, for the summary.
group_of() { # group_of PID COMM
    case $2 in
        # sdcompat*: installs from before the rename to uinitconverter.
        s6-log|systemd-journal|rsyslogd|uinitconverter-journa*|sdcompat-journal*) echo logging ;;
        s6-*|uinitconverter*|sdcompat*) echo init ;;
        systemd|'(sd-pam)')
            if [ "$1" = 1 ]; then echo init; else echo user-manager; fi ;;
        *) [ "$1" = 1 ] && echo init || echo other ;;
    esac
}

snap() {
    local wait=300 dir="" owner="" exclude=""
    while [ $# -gt 0 ]; do
        case $1 in
            --wait) [ $# -ge 2 ] || usage; wait=$2; shift 2 ;;
            --exclude-cgroup) [ $# -ge 2 ] || usage; exclude=$2; shift 2 ;;
            -*) usage ;;
            *) dir=$1; shift ;;
        esac
    done
    if [ -n "${SUDO_USER:-}" ] && [ "$(id -u)" = 0 ]; then
        owner=$SUDO_USER
    fi
    if [ -z "$dir" ]; then
        local home=$HOME
        [ -n "$owner" ] && home=$(getent passwd "$owner" | cut -d: -f6)
        dir=$home/uinitconverter-mem
    fi

    local up
    up=$(cut -d. -f1 /proc/uptime)
    if [ "$up" -lt "$wait" ]; then
        echo "Up ${up}s; waiting until ${wait}s so both boots settle the same time..." >&2
        sleep $((wait - up))
    fi
    [ "$(id -u)" = 0 ] || echo "Not root: PSS only for your own processes." >&2

    local pid1 init
    pid1=$(cat /proc/1/comm)
    case $pid1 in
        s6-svscan) init=s6 ;;
        systemd) init=systemd ;;
        *) init=$pid1 ;;
    esac

    mkdir -p "$dir" || exit 1
    local out
    out=$dir/mem-$init-$(date +%Y%m%d-%H%M%S).tsv

    {
        printf 'meta\tinit\t%s\n' "$init"
        printf 'meta\tdate\t%s\n' "$(date '+%F %T')"
        printf 'meta\tuptime_s\t%s\n' "$(cut -d. -f1 /proc/uptime)"
        printf 'meta\tkernel\t%s\n' "$(uname -r)"
        printf 'meta\troot\t%s\n' "$([ "$(id -u)" = 0 ] && echo yes || echo no)"
        printf 'meta\tjournald\t%s\n' "$(pgrep -x systemd-journal >/dev/null && echo running || echo no)"
        printf 'meta\tswap_devices\t%s\n' "$(($(wc -l </proc/swaps) - 1))"
        printf 'meta\trunning_services\t%s\n' \
            "$(timeout 10 systemctl list-units --type=service --state=running --no-legend 2>/dev/null | wc -l)"
        printf 'meta\tcmdline\t%s\n' "$(cat /proc/cmdline)"

        local k v
        while read -r k v; do
            printf 'mem\t%s\t%s\n' "${k%:}" "$(kb "$v")"
        done </proc/meminfo

        local d pid name rss anon shm pss kthreads=0
        for d in /proc/[0-9]*; do
            pid=${d#/proc/}
            name="" rss="" anon=0 shm=0 pss=-
            while IFS=$'\t' read -r k v; do
                case $k in
                    Name:) name=$v ;;
                    VmRSS:) rss=$(kb "$v") ;;
                    RssAnon:) anon=$(kb "$v") ;;
                    RssShmem:) shm=$(kb "$v") ;;
                esac
            done 2>/dev/null <"$d/status"
            [ -n "$name" ] || continue  # exited meanwhile
            if [ -n "$exclude" ]; then
                case $(cat "$d/cgroup" 2>/dev/null) in
                    */"$exclude"|*/"$exclude"/*) continue ;;
                esac
            fi
            if [ -z "$rss" ]; then      # kernel thread
                kthreads=$((kthreads + 1))
                continue
            fi
            while read -r k v; do
                [ "$k" = Pss: ] && { pss=$(kb "$v"); break; }
            done 2>/dev/null <"$d/smaps_rollup"
            printf 'proc\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$pid" "$name" "$(group_of "$pid" "$name")" "$rss" "$anon" "$shm" "$pss"
        done
        printf 'meta\tkernel_threads\t%s\n' "$kthreads"
    } >"$out"

    [ -n "$owner" ] && chown -R "$owner:" "$dir"
    echo "$out"
}

compare() {
    [ $# = 2 ] || usage
    local f
    for f in "$1" "$2"; do
        [ -r "$f" ] || { echo "cannot read $f" >&2; exit 1; }
    done
    awk -F'\t' '
    FNR == 1 { n++ }
    $1 == "meta" { meta[n, $2] = $3 }
    $1 == "mem"  { mem[n, $2] = $3 }
    $1 == "proc" {
        # proc pid comm group rss anon shmem pss
        c = $3; g = $4
        names[c] = 1; groups[g] = 1
        cnt[n, c]++; anon[n, c] += $6; rss[n, c] += $5
        gcnt[n, g]++; ganon[n, g] += $6
        procs[n]++; tanon[n] += $6
        if ($8 == "-") nopss[n] = 1
        else { pss[n, c] += $8; gpss[n, g] += $8; tpss[n] += $8 }
    }
    function mib(kb) { return sprintf("%.1f", kb / 1024) }
    function row(label, a, b) {
        printf "  %-26s %12s %12s %+12.1f\n", label, mib(a), mib(b), (b - a) / 1024
    }
    function used(i) {   # as free(1): total - free - buffers - cache - reclaimable slab
        return mem[i, "MemTotal"] - mem[i, "MemFree"] - mem[i, "Buffers"] \
             - mem[i, "Cached"] - mem[i, "SReclaimable"]
    }
    END {
        A = meta[1, "init"]; B = meta[2, "init"]
        usepss = !(1 in nopss) && !(2 in nopss)
        metric = usepss ? "PSS" : "private anon (RssAnon)"

        printf "  %-18s %-24s %s\n", "", A, B
        split("date uptime_s kernel root journald swap_devices running_services kernel_threads", mk, " ")
        for (i = 1; i in mk; i++)
            printf "  %-18s %-24s %s\n", mk[i], meta[1, mk[i]], meta[2, mk[i]]
        printf "  %-18s %-24s %s\n", "user processes", procs[1], procs[2]
        if (meta[1, "uptime_s"] - meta[2, "uptime_s"] > 120 || meta[2, "uptime_s"] - meta[1, "uptime_s"] > 120)
            print "  (uptimes differ by more than 2 minutes; the comparison is less fair)"

        printf "\nSystem memory (MiB)%9s %12s %12s %12s\n", "", A, B, "diff"
        row("used (as free)", used(1), used(2))
        row("total - available", mem[1, "MemTotal"] - mem[1, "MemAvailable"], mem[2, "MemTotal"] - mem[2, "MemAvailable"])
        split("AnonPages Shmem Slab SReclaimable SUnreclaim KernelStack PageTables Percpu VmallocUsed Buffers Cached", mf, " ")
        for (i = 1; i in mf; i++) row(mf[i], mem[1, mf[i]], mem[2, mf[i]])
        row("swap used", mem[1, "SwapTotal"] - mem[1, "SwapFree"], mem[2, "SwapTotal"] - mem[2, "SwapFree"])

        printf "\nBy group, %s (MiB)\n", metric
        printf "  %-14s %6s %6s %12s %12s %12s\n", "group", "n " A, "n " B, A, B, "diff"
        split("init logging user-manager other", go, " ")
        for (i = 1; i in go; i++) {
            g = go[i]
            a = usepss ? gpss[1, g] : ganon[1, g]; b = usepss ? gpss[2, g] : ganon[2, g]
            printf "  %-14s %6d %6d %12s %12s %+12.1f\n", g, gcnt[1, g], gcnt[2, g], mib(a), mib(b), (b - a) / 1024
        }
        a = usepss ? tpss[1] : tanon[1]; b = usepss ? tpss[2] : tanon[2]
        printf "  %-14s %6d %6d %12s %12s %+12.1f\n", "all", procs[1], procs[2], mib(a), mib(b), (b - a) / 1024

        # Commands sorted by the size of the difference.
        m = 0
        for (c in names) {
            a = usepss ? pss[1, c] : anon[1, c]; b = usepss ? pss[2, c] : anon[2, c]
            d = b - a; ad = d < 0 ? -d : d
            if (ad < 512) continue
            m++; key[m] = c; adiff[m] = ad
        }
        for (i = 2; i <= m; i++) {   # insertion sort, descending
            k = key[i]; v = adiff[i]
            for (j = i - 1; j >= 1 && adiff[j] < v; j--) { key[j + 1] = key[j]; adiff[j + 1] = adiff[j] }
            key[j + 1] = k; adiff[j + 1] = v
        }
        printf "\nCommands that differ by 0.5 MiB or more, %s (MiB), largest first\n", metric
        printf "  %-22s %6s %6s %12s %12s %12s\n", "command", "n " A, "n " B, A, B, "diff"
        for (i = 1; i <= m && i <= 40; i++) {
            c = key[i]
            a = usepss ? pss[1, c] : anon[1, c]; b = usepss ? pss[2, c] : anon[2, c]
            printf "  %-22s %6d %6d %12s %12s %+12.1f\n", substr(c, 1, 22), cnt[1, c], cnt[2, c], mib(a), mib(b), (b - a) / 1024
        }
        if (m > 40) printf "  ... %d more\n", m - 40
    }' "$1" "$2"
}

case ${1:-} in
    snap) shift; snap "$@" ;;
    compare) shift; compare "$@" ;;
    *) usage ;;
esac
