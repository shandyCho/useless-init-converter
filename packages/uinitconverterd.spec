# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverterd
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        The org.freedesktop.systemd1 D-Bus API for s6-rc
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
Requires:       s6-rc
Requires:       dbus-daemon
# Transient services (systemd-run) and the stop/reload commands run it.
Requires:       uinitconverter-runner = %{version}-%{release}
# Transient timers (systemd-run --on-calendar) run it.
Recommends:     uinitconverter-timers = %{version}-%{release}

%description
Serves the org.freedesktop.systemd1 D-Bus API on the system bus and on
/run/systemd/private, so systemctl, libsystemd and other clients keep working
with s6-rc: unit state, start/stop jobs, daemon-reload (it regenerates the
database itself), transient units and properties.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
cargo build --release --offline -p uinitconverterd

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverterd -p uinitconverter-base -p uinitconverter-client -p uinitconverter-exec -p uinitconverter-gen -p uinitconverter-install -p uinitconverter-version

%install
install -Dm0755 target/release/uinitconverterd %{buildroot}%{_libexecdir}/uinitconverter/uinitconverterd
install -Dm0644 dist/dbus/uinitconverter.conf %{buildroot}%{_datadir}/dbus-1/system.d/uinitconverter.conf

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverterd
%{_datadir}/dbus-1/system.d/uinitconverter.conf

%changelog
* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
