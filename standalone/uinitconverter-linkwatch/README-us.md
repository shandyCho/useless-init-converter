# uinitconverter-linkwatch

**English** | [한국어](README.md)

Unblocks `s6-rc-update` (a daemon-reload) and `s6-svlink` when they wait for a
new supervisor's start event that came before they listened.

A standalone tool split out of uinitconverter. It reads no configuration and builds,
installs and runs without the other uinitconverter parts. Its only dependency is
libc; what it watches is s6's.

## Why

`s6_supervise_link_names()` (s6 2.15), which both programs use, first notes
which service directories already have a supervisor, then subscribes to the
others' `event/` fifodirs, links them into the scan directory and waits for
each one's `s` event. If s6-svscan starts a supervisor in between (s6-rc-update
has just stopped the old one, s6-svscan respawns it and finds the new link),
the `s` goes to nobody. The wait never ends: s6-rc-update has no timeout unless
given `-t`, and it holds s6-rc's lock, so every s6-rc command after it waits
too.

## What it does

Every `--interval` seconds it looks in `/proc` for an `s6-ftrigrd` whose parent
is `s6-rc-update` or `s6-svlink` and which has run for `--after` seconds or
more (the subscribing takes milliseconds, so such a process is waiting). For
each `event/ftrig1:*` fifo that s6-ftrigrd holds, if the service directory's
`supervise/lock` is held (its supervisor runs, so its start event has been
sent), it writes `s` into that fifo, once. Nothing else is touched: other
programs' waits (s6-rc waiting for a service, s6-svwait) are left alone, and a
fifo whose event already came gets a duplicate that the waiter no longer looks
at.

It needs to read the waiting processes' `/proc/PID/fd`: run it as the same
user as they are (root on a real system).

## Build

```sh
cargo build --release
cargo test
```

## Install

```sh
sudo install -Dm0755 target/release/uinitconverter-linkwatch /usr/libexec/uinitconverter/uinitconverter-linkwatch
```

RPM: `./build-rpm.sh [OUTDIR]` builds `uinitconverter-linkwatch.spec` into
`~/rpmbuild-uinitconverter-standalone/rpms` (no sudo; each rebuild gets a newer release, so
`sudo dnf upgrade` takes it). Install it with
`sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/uinitconverter-linkwatch-*.rpm`. `./build-rpm.sh --srpm` makes only
the source RPM, to rebuild elsewhere (`rpmbuild --rebuild`, mock). uinitconverter runs it as the s6 service `uinitconverter-linkwatch`
(idle when the tool is not installed).

## Run

```sh
uinitconverter-linkwatch [--after SECONDS] [--interval SECONDS] [--under DIR] [--once]
```

| Option | Meaning |
|---|---|
| `--after SECONDS` | how long a wait lasts before it is unblocked (default 10) |
| `--interval SECONDS` | how often to look (default 2) |
| `--under DIR` | only service directories below DIR (for a test's own s6 tree) |
| `--once` | look once and exit |

Each event it sends is logged to stderr:
`uinitconverter-linkwatch: s6-rc-update (pid N) waited 12 s for DIR, whose supervisor runs: sent its start event`.
