#!/bin/sh
# Regenerate crates/sdcompat-exec/src/syscall-groups.txt (systemd's @groups for SystemCallFilter=)
# from the installed systemd. Run on Fedora after a systemd update.
set -eu
out=$(dirname "$0")/../crates/sdcompat-exec/src/syscall-groups.txt
{
    echo "# systemd's system call groups, from \`systemd-analyze syscall-filter\` of"
    echo "# $(rpm -q systemd 2>/dev/null || systemctl --version | head -n1). Regenerate with tests/update-syscall-groups.sh."
    systemd-analyze syscall-filter 2>/dev/null | grep -vE '^[[:space:]]*#' | sed 's/^[[:space:]]\{1,\}/    /'
} >"$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
