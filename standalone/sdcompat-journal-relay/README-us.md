# sdcompat-journal-relay

**English** | [한국어](README.md)

For systems that run without systemd-journald. Listens on journald's client
sockets (the native protocol on `/run/systemd/journal/socket` and the
`stdout` stream) and forwards what arrives to syslog (`/dev/log`, usually
rsyslog). Some programs, such as dbus-broker-launch, refuse to start when
those sockets are missing.

A standalone tool split out of sdcompat. It builds, installs and runs without
the other sdcompat parts. It needs libc, and at run time a syslog daemon
(rsyslog or another) on `/dev/log`.

## Build

```sh
cargo build --release
cargo test
```

## Install

```sh
sudo install -Dm0755 target/release/sdcompat-journal-relay /usr/libexec/sdcompat/sdcompat-journal-relay
```

RPM: `./build-rpm.sh [OUTDIR]` builds `sdcompat-journal-relay.spec` into
`~/rpmbuild-sdcompat-standalone/rpms` (no sudo; each rebuild gets a newer release, so
`sudo dnf upgrade` takes it). Install it with
`sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-journal-relay-*.rpm`; `setup-fedora.sh` then leaves
the tool to rpm.

## Run

```sh
sdcompat-journal-relay [--dir DIR] [--syslog PATH] [--ready-fd N]
```

- `--dir DIR`: where to create journald's sockets (default `/run/systemd/journal`).
- `--syslog PATH`: the syslog socket to forward to (default `/dev/log`).
- `--ready-fd N`: write a newline to fd N once listening (s6 readiness).

Don't run it where journald runs (they use the same sockets). With sdcompat
installed as `JOURNAL=no`, the `journal-relay` s6 service runs it, and then it
must be installed (`setup-fedora.sh --no-journal` checks).

Only MESSAGE, PRIORITY, SYSLOG_FACILITY, SYSLOG_IDENTIFIER and SYSLOG_PID are
forwarded; other fields are dropped. Messages are dropped too when syslog is
not up or does not keep up, so that clients never block.
