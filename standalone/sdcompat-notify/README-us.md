# sdcompat-notify

**English** | [한국어](README.md)

A `systemd-notify` replacement. Sends one sd_notify message to the socket in
`$NOTIFY_SOCKET` (a path, or `@name` for the abstract namespace). Works under
any service manager that sets it (systemd, sdcompat, ...).

A standalone tool split out of sdcompat. It builds, installs and runs without
the other sdcompat parts. Its only dependency is libc.

## Build

```sh
cargo build --release
```

## Install

```sh
sudo install -Dm0755 target/release/sdcompat-notify /usr/libexec/sdcompat/sdcompat-notify
```

RPM: `./build-rpm.sh [OUTDIR]` builds `sdcompat-notify.spec` into
`~/rpmbuild-sdcompat-standalone/rpms` (no sudo; each rebuild gets a newer release, so
`sudo dnf upgrade` takes it). Install it with
`sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-notify-*.rpm`. `./build-rpm.sh --srpm` makes only
the source RPM, for a build service such as COPR.

## Run

```sh
sdcompat-notify [OPTIONS] [VAR=VALUE...]
sdcompat-notify --exec [OPTIONS] [VAR=VALUE...] ";" COMMAND [ARGS...]
sdcompat-notify --booted
```

| Option | Sends / does |
|---|---|
| `--ready`, `--stopping` | `READY=1`, `STOPPING=1` |
| `--reloading` | `RELOADING=1` and `MONOTONIC_USEC=` |
| `--status=TEXT` | `STATUS=TEXT` |
| `--pid[=PID]` | `MAINPID=PID`; `auto` or `parent` (the default) is the caller, `self` this process |
| `--uid=USER` | sends as USER |
| `--fd=N`, `--fdname=NAME` | stores fd N with the manager (`FDSTORE=1`, `FDNAME=`); `--fd` may repeat |
| `--no-block` | doesn't wait for the manager to read the message |
| `--exec` | runs COMMAND after sending, in place of itself |
| `--booted` | exit code 0 if `/run/systemd/system` exists, otherwise 1 |

As systemd-notify (254 and later), the message goes out on behalf of the
calling process (its PID in the credentials, where the kernel allows it: root
may name another process) and then waits with `BARRIER=1` until the manager
has read it (5 s at most; without an answer it only warns).

For example, a service script saying it is ready:

```sh
sdcompat-notify --ready --status="listening on 8080"
```

Without `$NOTIFY_SOCKET` it fails (exit code 1).
