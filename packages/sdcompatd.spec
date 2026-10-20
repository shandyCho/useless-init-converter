# One of the sdcompat RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under sdcompat-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompatd
Version:        0.1.0
Release:        1%{?dist}
Summary:        The org.freedesktop.systemd1 D-Bus API for s6-rc
# Choose a license before distributing.
License:        TBD
Source0:        sdcompat-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
Requires:       s6-rc
Requires:       dbus-daemon
# Transient services (systemd-run) and the stop/reload commands run it.
Requires:       sdcompat-runner = %{version}-%{release}
# Transient timers (systemd-run --on-calendar) run it.
Recommends:     sdcompat-timers = %{version}-%{release}

%description
Serves the org.freedesktop.systemd1 D-Bus API on the system bus and on
/run/systemd/private, so systemctl, libsystemd and other clients keep working
with s6-rc: unit state, start/stop jobs, daemon-reload (it regenerates the
database itself), transient units and properties.

%prep
%autosetup -n sdcompat-%{version}

%build
cargo build --release --offline -p sdcompatd

%check
cargo test --release --offline -p sdcompatd

%install
install -Dm0755 target/release/sdcompatd %{buildroot}%{_libexecdir}/sdcompat/sdcompatd
install -Dm0644 dist/dbus/sdcompat.conf %{buildroot}%{_datadir}/dbus-1/system.d/sdcompat.conf

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompatd
%{_datadir}/dbus-1/system.d/sdcompat.conf

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of the sdcompat package
