#!/usr/bin/python3 -I
"""Drive a QEMU guest's serial console for tests/initramfs.sh.

    qemu-console.py LOG TIMEOUT STEP... -- QEMU-COMMAND...

Each STEP is "wait:REGEX" (wait for REGEX in the console output),
"send:TEXT" (type TEXT and a newline) or "sleep:SECONDS" (cryptsetup
prints its prompt before turning echo off with TCSAFLUSH, which drops what
was typed in between: pause before answering it). The steps run in
order; after the last one the guest must power off within TIMEOUT seconds. The console is
copied to LOG. Exit 0 when every step matched and the guest stopped, 1
otherwise (the missing step is printed).
"""
import os
import re
import select
import subprocess
import sys
import time


def main():
    log_path, timeout = sys.argv[1], float(sys.argv[2])
    sep = sys.argv.index("--")
    steps, cmd = sys.argv[3:sep], sys.argv[sep + 1:]
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    out = b""
    seen = 0  # output before this offset has been matched already
    deadline = time.monotonic() + timeout
    with open(log_path, "wb") as log:

        def read(until):
            nonlocal out
            while time.monotonic() < until:
                r, _, _ = select.select([proc.stdout], [], [], 0.2)
                if r:
                    chunk = os.read(proc.stdout.fileno(), 65536)
                    if not chunk:
                        return False
                    out += chunk
                    log.write(chunk)
                    log.flush()
                    return True
                if proc.poll() is not None:
                    return False
            return False

        for step in steps:
            kind, _, arg = step.partition(":")
            if kind == "send":
                proc.stdin.write(arg.encode() + b"\n")
                proc.stdin.flush()
                continue
            if kind == "sleep":
                until = time.monotonic() + float(arg)
                while time.monotonic() < until and read(until):
                    pass
                continue
            pat = re.compile(arg.encode())
            while True:
                m = pat.search(out, seen)
                if m:
                    seen = m.end()
                    break
                if not read(deadline):
                    print(f"qemu-console: not seen: {arg}", file=sys.stderr)
                    proc.kill()
                    return 1
        while proc.poll() is None and time.monotonic() < deadline:
            read(deadline)
        if proc.poll() is None:
            print("qemu-console: the guest did not power off", file=sys.stderr)
            proc.kill()
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
