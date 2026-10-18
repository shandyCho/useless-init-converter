# Build from a tarball that contains vendored crates:
#   cargo vendor && tar czf sdcompat-0.1.0.tar.gz --transform 's,^,sdcompat-0.1.0/,' \
#       Cargo.toml Cargo.lock src dist vendor .cargo
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat
Version:        0.1.0
Release:        1%{?dist}
Summary:        systemd compatibility layer for s6-rc
# Choose a license before distributing.
License:        TBD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
Requires:       s6-rc
Requires:       execline
Requires:       dbus-daemon
Requires:       util-linux
Recommends:     s6-linux-init
Recommends:     elogind
Recommends:     rsyslog

%description
Translates systemd unit files into an s6-rc database, runs services with
systemd's execution semantics, and serves the org.freedesktop.systemd1 D-Bus
API (system bus and /run/systemd/private) so systemctl, libsystemd and other
clients keep working with s6 as the init system.

%prep
%autosetup

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm0755 target/release/sdcompat %{buildroot}%{_libexecdir}/sdcompat/sdcompat
install -Dm0755 dist/early %{buildroot}%{_libexecdir}/sdcompat/early
install -d %{buildroot}%{_bindir}
ln -s ../libexec/sdcompat/sdcompat %{buildroot}%{_bindir}/sdcompat
install -Dm0644 dist/dbus/sdcompat.conf %{buildroot}%{_datadir}/dbus-1/system.d/sdcompat.conf
install -Dm0644 dist/sdcompat.conf %{buildroot}%{_sysconfdir}/sdcompat/sdcompat.conf
install -d %{buildroot}%{_sysconfdir}/sdcompat
cp -a dist/s6 %{buildroot}%{_sysconfdir}/sdcompat/s6
install -Dm0755 dist/s6-linux-init/rc.init %{buildroot}%{_datadir}/sdcompat/s6-linux-init/rc.init
install -Dm0755 dist/s6-linux-init/init-selinux %{buildroot}%{_sbindir}/init-selinux
install -d %{buildroot}%{_sharedstatedir}/sdcompat %{buildroot}%{_localstatedir}/log/sdcompat

%files
%{_bindir}/sdcompat
%{_libexecdir}/sdcompat/
%{_sbindir}/init-selinux
%{_datadir}/dbus-1/system.d/sdcompat.conf
%{_datadir}/sdcompat/
%dir %{_sysconfdir}/sdcompat
%config(noreplace) %{_sysconfdir}/sdcompat/sdcompat.conf
%config(noreplace) %{_sysconfdir}/sdcompat/s6/
%dir %{_sharedstatedir}/sdcompat
%dir %{_localstatedir}/log/sdcompat

%changelog
* Sun Oct 18 2026 sdcompat developers - 0.1.0-1
- Initial package
