#!/bin/sh
# Check the cgroup BPF programs (DevicePolicy=, IPAddressDeny=) in the real
# kernel. Loading and attaching them needs root, so this builds the test
# binary as you and runs only that test under sudo. It works in a scratch
# cgroup /sys/fs/cgroup/sdcompat-bpf-test-PID, removed at the end; no unit
# and no init system is touched.
#
#   tests/bpf-root.sh
set -eu

cd "$(dirname "$0")/.."
bin=$(cargo test -p sdcompat-exec --lib --no-run --message-format=json 2>/dev/null |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -n 1)
[ -x "$bin" ] || { echo "cannot find the test binary (cargo test -p sdcompat-exec --lib --no-run)" >&2; exit 1; }
exec sudo "$bin" --ignored --exact bpf::tests::kernel_enforces --nocapture
