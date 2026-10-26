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
  own spec and `build-rpm.sh` (result: `~/rpmbuild-sdcompat-standalone/rpms`), and
  `sdcompat` only points at them with `Recommends`/`Suggests`.

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

## Versions

The specs carry no version. The script sets it, so build the specs with
this script (plain `rpmbuild` fails for lack of a version).

- **Version** is read from one place, `[workspace.package] version` in
  `Cargo.toml`. The crates use it too, so a version bump is that one line.
- **Release** is new for every build: `1.YYYYMMDDHHMMSS.gCOMMIT`, with
  `.dirty` appended when there are uncommitted changes. Example:
  `sdcompat-gen-0.1.0-1.20261021071849.g0023111.fc45`. The build time comes
  first, so when only the Rust code changed and the version did not, dnf
  still sees the new build as an upgrade.
- The five packages are tied to one version-release, and the script always
  builds all five together. `rpms/` holds only the last build's five.
- The tools built as RPMs print the package's version-release for
  `--version` (e.g. `sdcompat-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`);
  built from source they print the commit, like `0.1.0 (git c8aabf8)`.
- To set the release yourself, e.g. for a published release:
  `SDCOMPAT_RELEASE=1 packages/build-rpms.sh`.

## Installing

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet tools (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

After the Rust code changed, rebuild and upgrade:

```sh
git pull                                            # or make your change and commit it
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm
sdcompat --version                                  # which build is installed
```

On a running s6 boot, the new sdcompatd is used once the daemon restarts
(`s6-svc -r /run/service/sdcompatd`). The runner and timers are used the next
time a service starts, and sdcompat-gen at the next `sdcompat reload`
(`systemctl daemon-reload` runs sdcompatd's copy, so it changes once the
daemon restarts). Nothing is regenerated at boot.

Moving from a version without the s6-rc repository (before the `s6 set`
change): rerun `dist/setup-fedora.sh`. It removes `/etc/sdcompat/s6/default`
(the set's commit makes the `default` bundle now; a reload refuses to run
while it exists), writes the new `/etc/s6.conf`, creates `/etc/s6/sources`,
loads the new SELinux module and creates the repository with the set
`current` from systemd's enable state.

The s6-linux-init skeleton (`rc.init`, `rc.shutdown`, `rc.shutdown.final`)
is different: the package installs the copy setup works from, but the boot
uses `/etc/s6-linux-init/current`, which `s6-linux-init-maker` makes from it.
When an upgrade changes the skeleton, rerun `sudo dist/setup-fedora.sh` (with
the options of the first run) to rebuild it; `early` and the rest take effect
at the next boot without that.

When the `sdcompat` RPM is installed, `setup-fedora.sh` uses it instead of
building the core. When only some tool RPMs are installed, it leaves those
tools alone and installs the rest from source. To remove:
`dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`.
