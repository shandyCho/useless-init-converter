# sdcompat-fdstore

**English** | [한국어](README.md)

A small server that holds file descriptors for services. Like systemd's
`FDSTORE=1` and `FileDescriptorStoreMax=`, it keeps the fds services hand over,
and the listening sockets of socket-activated services, in its own process and
gives them back after the service or the service manager restarts.

A standalone tool split out of sdcompat. It reads no configuration and no
unit files, and builds, installs and runs without the other sdcompat parts.
Its only dependency is libc.

## Build

```sh
cargo build --release
cargo test
```

## Install

```sh
sudo install -Dm0755 target/release/sdcompat-fdstore /usr/libexec/sdcompat/sdcompat-fdstore
```

RPM: `./build-rpm.sh [OUTDIR]` builds `sdcompat-fdstore.spec` into
`~/rpmbuild-sdcompat-standalone/rpms` (no sudo; each rebuild gets a newer release, so
`sudo dnf upgrade` takes it). Install it with
`sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-fdstore-*.rpm`; `setup-fedora.sh` then leaves
the tool to rpm.

## Run

```sh
sdcompat-fdstore [--socket PATH] [--ready-fd N]
```

- `--socket PATH`: where to listen (default `/run/sdcompat/fdstore`).
- `--ready-fd N`: write a newline to fd N once listening (s6 readiness).

Only root and the user running the server may use it, and under SELinux only
peers in the server's own domain (with sdcompat: `init_t`, the domain of
sdcompatd and the runner, its only clients; many confined root daemons may
connect to `init_t`'s sockets). With sdcompat, the
`/etc/sdcompat/s6/sdcompat-fdstore` s6 service runs it; when this tool is not
installed that service idles and sdcompatd holds the fds itself.

## Protocol

A `SOCK_SEQPACKET` socket, one fd per message (SCM_RIGHTS):

| Request | Reply |
|---|---|
| `PUT\0<key>\0<name>\0<max>` + fd | `OK` or `ERR <msg>` |
| `GET\0<key>` | `FD\0<name>` + fd for each, then `END` |
| `DEL\0<key>[\0<name>]` | `OK` |

`<max>` is how many fds the key may hold (the service's
`FileDescriptorStoreMax=`). The client sends it so the server needs no unit
files. Without it only the `<unit>#sockets` (1024) and `<unit>#stdio` (3) keys
are accepted.
