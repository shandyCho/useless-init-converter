#!/bin/bash
# Every automatic test, with one result.
#
#   tests/all.sh             # the unprivileged tests in podman, then the VM test
#   tests/all.sh --host      # the unprivileged tests on this machine instead
#   tests/all.sh --no-vm     # without tests/confine-vm.sh
#   tests/all.sh --vm-only   # only tests/confine-vm.sh
#
# Exit status 0 only if everything passed. Logs are kept in LOGDIR (default: a new
# directory under ${TMPDIR:-/tmp}, printed at the end). Nothing here touches the
# host's init system or needs root. What these can't show is still a real boot:
# SELinux enforcing, the real services, the hardware (TESTING.md's real boot checks).
set -uo pipefail

TOP=$(cd "$(dirname "$0")/.." && pwd)
cd "$TOP" || exit 1

where=container vm=1 fast=1
while [ $# -gt 0 ]; do
    case $1 in
        --host) where=host ;;
        --inner) where=here vm=0 ;;
        --no-vm) vm=0 ;;
        --vm-only) fast=0 ;;
        *) sed -n '2,11s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
    esac
    shift
done

LOGDIR=${LOGDIR:-$(mktemp -d "${TMPDIR:-/tmp}/uinitconverter-all.XXXXXX")}
mkdir -p "$LOGDIR"
failed=0
results=()

# step NAME COMMAND...: its output goes to the log, its last lines to the screen
# if it fails.
step() {
    local name=$1 log rc summary skips
    shift
    log=$LOGDIR/${name//[^a-zA-Z0-9_.=-]/_}.log
    echo "--- $name"
    "$@" >"$log" 2>&1
    rc=$?
    summary=$(awk '
        /^passed: / { line = $0 }
        /^test result: / { n++; p += $4; f += $6 }
        END { if (line != "") print line; else if (n) print "passed: " p "  failed: " f }' "$log")
    skips=$(grep -ac '^SKIP ' "$log")
    [ "$skips" = 0 ] && skips="" || skips=" ($skips skipped)"
    if [ "$rc" = 0 ]; then
        results+=("PASS  $name: $summary$skips")
    else
        failed=1
        results+=("FAIL  $name: exit $rc $summary$skips")
        grep -aE '^(FAIL|not ok)|FAILED|panicked|^error' "$log" | head -n 20
        tail -n 15 "$log"
    fi
}

# A shell script by what its first line says, as many have no .sh.
shell_scripts() {
    local f first
    while IFS= read -r -d '' f; do
        IFS= read -r first <"$f" || true
        case $first in
            '#!/bin/sh'*|'#!/bin/bash'*|'#!/usr/bin/env bash'*|'#!/usr/bin/sh'*|'#!/usr/bin/bash'*) printf '%s\0' "$f" ;;
        esac
    done < <(find . \( -name target -o -name .git \) -prune -o -type f -print0)
}

check_shell() {
    local f rc=0
    while IFS= read -r -d '' f; do
        bash -n "$f" || rc=1
        shellcheck -S warning "$f" || rc=1
    done < <(shell_scripts)
    return "$rc"
}

# The user-mode checks (tests/user-sandbox.sh), in a directory of their own.
user_sandbox() {
    local w rc
    w=$(mktemp -d "${TMPDIR:-/tmp}/sdcu.XXXXXX") || return 1
    tests/user-sandbox.sh "$w"
    rc=$?
    rm -rf "$w"
    return "$rc"
}

# initconvert-user-setup against a fake /sys and /proc (tests/user-setup.sh).
user_setup() {
    local w rc
    w=$(mktemp -d "${TMPDIR:-/tmp}/sdcs.XXXXXX") || return 1
    tests/user-setup.sh "$w"
    rc=$?
    rm -rf "$w"
    return "$rc"
}

# A work directory of its own, under a short path for the suite's sockets.
sandbox_suite() {
    local w rc
    w=$(mktemp -d "${TMPDIR:-/tmp}/sdc.XXXXXX") || return 1
    rm -rf "$w"
    if tests/sandbox.sh "$w"; then
        tests/check.sh "$w"
        rc=$?
    else
        rc=1
    fi
    tests/sandbox.sh "$w" stop
    rm -rf "$w"
    return "$rc"
}

sandbox_with() (
    export "${1?}"
    sandbox_suite
)

standalone_test() {
    cargo test --manifest-path "standalone/$1/Cargo.toml"
}

fast_tests() {
    local d
    step shell check_shell
    step "cargo test" cargo test
    for d in standalone/*/; do
        d=${d%/}
        step "${d##*/}" standalone_test "${d##*/}"
    done
    step hardencompare-check tests/hardencompare-check.sh
    step memtest-check tests/memtest-check.sh
    if ! cargo build --quiet; then
        results+=("FAIL  cargo build")
        failed=1
        return
    fi
    step "sandbox" sandbox_suite
    step "sandbox JOURNAL=no" sandbox_with SANDBOX_JOURNAL=no
    step "sandbox FDSTORE=no" sandbox_with SANDBOX_FDSTORE=no
    step "sandbox BUS=broker" sandbox_with SANDBOX_BUS=broker
    step "user sandbox" user_sandbox
    step "user setup" user_setup
}

vm_test() {
    cargo build --quiet || return 1
    tests/confine-vm.sh "$LOGDIR/vm"
}

if [ "$fast" = 1 ]; then
    case $where in
        container)
            echo "=== unprivileged tests, in podman"
            step container tests/container.sh run
            ;;
        host|here)
            fast_tests
            ;;
    esac
fi
if [ "$vm" = 1 ]; then
    echo "=== real root, in QEMU"
    step "confine-vm" vm_test
fi

echo
printf '%s\n' "${results[@]}"
echo "logs: $LOGDIR"
if [ "$where" = container ] && [ "$fast" = 1 ]; then
    echo "per-test results: $LOGDIR/container.log"
fi
if [ "$failed" = 0 ]; then
    echo "Result: PASS"
else
    echo "Result: FAIL"
fi
exit "$failed"
