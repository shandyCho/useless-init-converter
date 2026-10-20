# sdcompat RPM packages

[한국어](README.md)

Each executable of the core (`crates/`) is its own RPM package. The source
stays in the one Cargo workspace, and all five packages build from the same
source tarball. The library crates (`sdcompat-base`, `sdcompat-install`,
`sdcompat-exec`, `sdcompat-client`) are linked statically into the
executables, so they are used only to build and are not packaged.

| Package | Contains | Needs at run time (`Requires`) |
|---|---|---|
| `sdcompat-runner` | `sdcompat-runner` | s6, shadow-utils. Recommends `sdcompatd` (`OnFailure=`/`OnSuccess=` units are started through the daemon) |
| `sdcompatd` | `sdcompatd`, the D-Bus policy (`sdcompat.conf`) | s6, s6-rc, dbus-daemon, `sdcompat-runner`. Recommends `sdcompat-timers` (`systemd-run --on-calendar`) |
| `sdcompat-timers` | `sdcompat-timers` | `sdcompatd` (timers start units through the daemon) |
| `sdcompat-gen` | `sdcompat-gen`, `/var/lib/sdcompat` | s6-rc, `sdcompat-runner`, `sdcompat-timers` (the generated services run them) |
| `sdcompat` | the `sdcompat` command, `early`, configuration, s6 services, s6-linux-init skeleton, SELinux module, kernel-install plugin, `/var/log/sdcompat` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, and the four packages above |

- `dnf install sdcompat` installs everything. The boot configuration and the
  SELinux module are in the `sdcompat` package, so booting with s6 needs it.
- The tool packages can be updated on their own. They are tied to the same
  version (`= version-release`).
- The standalone tools (`standalone/`) are not here. Each directory has its
  own spec, and `sdcompat` only points at them with `Recommends`/`Suggests`.

## Building

```sh
packages/build-rpms.sh            # result: ~/rpmbuild-sdcompat/rpms
packages/build-rpms.sh DIR        # somewhere else
```

The script packs the workspace (`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)
and the crates fetched with `cargo vendor` into one `sdcompat-VERSION.tar.gz`,
then runs `rpmbuild -bb` for each spec. rpmbuild builds without network
(`--offline`). Nothing is installed and no sudo is used. Needs `rpm-build`,
`cargo` and `rust`.

The five specs must have the same `Version:` (they share one tarball); the
script stops if they don't.

## Installing

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet tools (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

When the `sdcompat` RPM is installed, `setup-fedora.sh` uses it instead of
building the core. When only some tool RPMs are installed, it leaves those
tools alone and installs the rest from source. To remove:
`dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`.
