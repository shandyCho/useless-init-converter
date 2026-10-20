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

RPM: `sdcompat-notify.spec`.

## Run

```sh
sdcompat-notify [--ready] [--stopping] [--reloading] [--status=TEXT] [--pid=PID] [VAR=VALUE...]
```

For example, a service script saying it is ready:

```sh
sdcompat-notify --ready --status="listening on 8080"
```

Without `$NOTIFY_SOCKET` it fails (exit code 1).
