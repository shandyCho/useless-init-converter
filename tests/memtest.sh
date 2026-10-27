#!/bin/bash
# Memory comparison of the systemd boot and the s6 boot on this machine,
# taken on its own at every boot: reboot into either entry whenever you
# like, and the report compares what the boots measured so far.
#
#   sudo tests/memtest.sh install [--wait SECS] [--dir DIR] [--max-diff MIB]
#   sudo tests/memtest.sh uninstall
#   sudo tests/memtest.sh run [--wait SECS] [--dir DIR] [--max-diff MIB]
#   tests/memtest.sh report [DIR] [--max-diff MIB]
#
# install adds uinitconverter-memtest.service, enabled for systemd and in
# the s6 database. On every boot it waits SECS seconds (default 600, counted
# from its own start, so the time at the LUKS prompt doesn't count), takes a
# tests/memcompare.sh snapshot into DIR (default ~/uinitconverter-mem/memtest
# of the user who ran sudo) and rewrites DIR/report.txt. For a fair
# comparison don't log in until it is done: both inits are then measured at
# the GDM greeter (or log in and open the same apps every time). Its log is
# /var/lib/uinitconverter-memtest/log.
# run takes a snapshot now, by hand (it waits until SECS after boot).
# report prints the median of each init's snapshots in DIR and passes
# (exit 0) when s6's "total - available" is at most MIB (default 50) above
# systemd's. Delete old snapshots from DIR to start over.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
UNIT=uinitconverter-memtest.service
# MEMTEST_* overrides are for tests/memtest-check.sh.
LIB=${MEMTEST_LIB:-/usr/local/libexec/uinitconverter-memtest}
LOG_DIR=${MEMTEST_LOG_DIR:-/var/lib/uinitconverter-memtest}
UNIT_DIR=${MEMTEST_UNIT_DIR:-/etc/systemd/system}

usage() {
    sed -n '2,22s/^# \{0,1\}//p' "$0" >&2
    exit 2
}

die() {
    echo "memtest: $*" >&2
    exit 1
}

need_root() {
    [ "$(id -u)" = 0 ] || [ -n "${MEMTEST_TEST:-}" ] || die "$1 needs root (sudo)"
}

current_init() {
    if [ -n "${MEMTEST_INIT:-}" ]; then
        echo "$MEMTEST_INIT"
        return
    fi
    case $(cat /proc/1/comm) in
        s6-svscan) echo s6 ;;
        systemd) echo systemd ;;
        *) cat /proc/1/comm ;;
    esac
}

# Options shared by install, run and boot.
WAIT=600 DIR="" MAX=50 OWNER=${SUDO_USER:-$(id -un)}
parse_opts() {
    while [ $# -gt 0 ]; do
        case $1 in
            --wait) [ $# -ge 2 ] || usage; WAIT=$2; shift 2 ;;
            --dir) [ $# -ge 2 ] || usage; DIR=$2; shift 2 ;;
            --max-diff) [ $# -ge 2 ] || usage; MAX=$2; shift 2 ;;
            --owner) [ $# -ge 2 ] || usage; OWNER=$2; shift 2 ;;
            *) usage ;;
        esac
    done
    case $WAIT$MAX in *[!0-9]* | "") die "--wait and --max-diff take whole numbers" ;; esac
    [ -n "$DIR" ] || DIR=$(getent passwd "$OWNER" | cut -d: -f6)/uinitconverter-mem/memtest
}

# The s6 database follows unit files only on a reload: on the systemd boot
# the boot database, on the s6 boot the daemon's reload.
reload_db() {
    systemctl daemon-reload
    if [ "$(current_init)" != s6 ]; then
        uinitconverter reload --no-update >/dev/null
    fi
}

install() {
    parse_opts "$@"
    need_root install
    command -v uinitconverter >/dev/null || die "uinitconverter not found"
    case $DIR in *[[:space:]]*) die "--dir must not contain spaces (it goes into ExecStart=)" ;; esac
    mkdir -p "$LIB" "$LOG_DIR" "$DIR" || exit 1
    chown "$OWNER:" "$DIR" 2>/dev/null
    cp "$HERE/memtest.sh" "$HERE/memcompare.sh" "$LIB/" && chmod 755 "$LIB/memtest.sh" "$LIB/memcompare.sh" || exit 1
    # bin_t: a script under /home can't be run by a service with SELinux enforcing.
    if command -v restorecon >/dev/null; then
        restorecon -R "$LIB"
    fi
    cat >"$UNIT_DIR/$UNIT" <<EOF || exit 1
[Unit]
Description=uinitconverter memory snapshot (tests/memtest.sh)

[Service]
Type=simple
ExecStart=$LIB/memtest.sh boot --wait $WAIT --dir $DIR --max-diff $MAX --owner $OWNER
StandardOutput=append:$LOG_DIR/log
StandardError=append:$LOG_DIR/log

[Install]
WantedBy=multi-user.target
EOF
    # Enabled for systemd, and seen enabled the first time the s6 database
    # has it, so the s6 set makes it active too.
    systemctl enable "$UNIT" >/dev/null 2>&1 || die "systemctl enable $UNIT failed"
    reload_db
    echo "Installed: every boot takes a snapshot ${WAIT} s after it is up, into $DIR."
    echo "Report: $DIR/report.txt (or tests/memtest.sh report). Remove: sudo tests/memtest.sh uninstall"
}

uninstall() {
    need_root uninstall
    systemctl disable "$UNIT" >/dev/null 2>&1
    rm -f "$UNIT_DIR/$UNIT"
    rm -rf "$LIB"
    reload_db
    echo "Removed $UNIT; snapshots and reports are kept."
}

# Take a snapshot into DIR and rewrite DIR/report.txt. $1: memcompare's
# --wait (seconds of uptime).
snapshot() {
    local out rc
    mkdir -p "$DIR" || exit 1
    out=$("$HERE/memcompare.sh" snap --wait "$1" --exclude-cgroup "$UNIT" "$DIR") || die "snapshot failed"
    echo "$(date '+%F %T') $(current_init): $out"
    report "$DIR" --max-diff "$MAX" >"$DIR/report.txt" 2>&1
    rc=$?
    chown -R "$OWNER:" "$DIR" 2>/dev/null
    case $rc in
        0) echo "report: PASS ($DIR/report.txt)" ;;
        1) echo "report: FAIL ($DIR/report.txt)" ;;
        *) echo "report: needs snapshots of both inits ($DIR/report.txt)" ;;
    esac
}

# The boot service.
boot() {
    parse_opts "$@"
    echo "$(date '+%F %T') $(current_init) boot: snapshot in ${WAIT} s"
    sleep "$WAIT"
    snapshot 0
}

run() {
    parse_opts "$@"
    need_root run
    snapshot "$WAIT"
}

# Medians per init and the pass/fail verdict.
report() {
    local run="" max=50
    while [ $# -gt 0 ]; do
        case $1 in
            --max-diff) [ $# -ge 2 ] || usage; max=$2; shift 2 ;;
            -*) usage ;;
            *) run=$1; shift ;;
        esac
    done
    if [ -z "$run" ]; then
        parse_opts
        run=$DIR
    fi
    local -a files
    mapfile -t files < <(compgen -G "$run/*.tsv" | sort)
    [ ${#files[@]} -gt 0 ] || die "no snapshots in $run"
    awk -F'\t' -v max="$max" '
    FNR == 1 { n++; file[n] = FILENAME; sub(".*/", "", file[n]) }
    $1 == "meta" && $2 == "init" { init[n] = $3 }
    $1 == "meta" && $2 == "uptime_s" { up[n] = $3 }
    $1 == "mem" { mem[n, $2] = $3 }
    $1 == "proc" {
        procs[n]++
        if ($4 == "user-manager") um[n]++
        if ($8 == "-") nopss = 1
        else { pss[n, "all"] += $8; pss[n, $4] += $8 }
    }
    function v(i, k) {
        if (k == "used") return mem[i, "MemTotal"] - mem[i, "MemFree"] - mem[i, "Buffers"] - mem[i, "Cached"] - mem[i, "SReclaimable"]
        if (k == "unavail") return mem[i, "MemTotal"] - mem[i, "MemAvailable"]
        if (k ~ /^pss_/) return pss[i, substr(k, 5)]
        if (k == "procs") return procs[i] * 1024
        return mem[i, k]
    }
    function median(who, k,    m, i, j, a, t) {
        m = 0
        for (i = 1; i <= n; i++) if (init[i] == who) a[++m] = v(i, k)
        if (m == 0) return ""
        for (i = 2; i <= m; i++) { t = a[i]; for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]; a[j + 1] = t }
        return m % 2 ? a[(m + 1) / 2] : (a[m / 2] + a[m / 2 + 1]) / 2
    }
    function fmt(x, unit) { return x == "" ? "-" : sprintf(unit == "n" ? "%d" : "%.1f", x / 1024) }
    function row(label, k, unit,    a, b) {
        a = median("systemd", k); b = median("s6", k)
        printf "  %-24s %10s %10s %10s\n", label, fmt(a, unit), fmt(b, unit),
            a == "" || b == "" ? "-" : sprintf(unit == "n" ? "%+d" : "%+.1f", (b - a) / 1024)
    }
    END {
        for (i = 1; i <= n; i++) cnt[init[i]]++
        printf "Memory comparison: %d systemd and %d s6 snapshots, median of each (MiB)\n", cnt["systemd"], cnt["s6"]
        if (nopss) print "  (some snapshots were taken without root: PSS rows are incomplete)"
        printf "  %-24s %10s %10s %10s\n", "", "systemd", "s6", "diff"
        row("total - available", "unavail")
        row("used (as free)", "used")
        row("AnonPages", "AnonPages"); row("Shmem", "Shmem"); row("SUnreclaim", "SUnreclaim")
        row("KernelStack", "KernelStack"); row("PageTables", "PageTables")
        row("PSS, all processes", "pss_all"); row("PSS, init", "pss_init")
        row("PSS, logging", "pss_logging"); row("PSS, user managers", "pss_user-manager")
        row("PSS, other", "pss_other")
        row("processes (count)", "procs", "n")

        print "\nPer snapshot (MiB):"
        printf "  %-32s %-8s %7s %12s %9s %9s %7s\n", "file", "init", "uptime", "total-avail", "used", "PSS all", "usr-mgr"
        for (i = 1; i <= n; i++)
            printf "  %-32s %-8s %7d %12.1f %9.1f %9.1f %7d\n", file[i], init[i], up[i], v(i, "unavail") / 1024, v(i, "used") / 1024, pss[i, "all"] / 1024, um[i]

        if (!cnt["systemd"] || !cnt["s6"]) { print "\nResult: INCOMPLETE (needs snapshots of both inits)"; exit 2 }
        d = (median("s6", "unavail") - median("systemd", "unavail")) / 1024
        printf "\nResult: %s: s6 total - available %+.1f MiB against systemd (limit %+d MiB)\n", d <= max ? "PASS" : "FAIL", d, max
        exit d <= max ? 0 : 1
    }' "${files[@]}"
    local rc=$?
    # The details of the latest pair.
    local a="" b="" f
    for f in "${files[@]}"; do
        case $(awk -F'\t' '$1 == "meta" && $2 == "init" { print $3; exit }' "$f") in
            systemd) a=$f ;;
            s6) b=$f ;;
        esac
    done
    if [ -n "$a" ] && [ -n "$b" ]; then
        printf '\nLatest pair in detail (%s, %s):\n' "${a##*/}" "${b##*/}"
        "$HERE/memcompare.sh" compare "$a" "$b"
    fi
    return $rc
}

case ${1:-} in
    install) shift; install "$@" ;;
    uninstall) shift; uninstall ;;
    run) shift; run "$@" ;;
    boot) shift; boot "$@" ;;
    report) shift; report "$@" ;;
    *) usage ;;
esac
