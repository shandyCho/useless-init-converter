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
Requires:       s6-linux-init
Requires:       s6-rc
Requires:       execline
Requires:       dbus-daemon
Requires:       util-linux
Requires(post): policycoreutils
Requires(postun): policycoreutils
Recommends:     s6-linux-init
Recommends:     rsyslog
Recommends:     earlyoom
# The s6 command; setup-fedora.sh writes /etc/s6.conf for it.
Recommends:     s6-frontend

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
install -d %{buildroot}%{_datadir}/sdcompat/s6-linux-init
cp -a dist/s6-linux-init/skel %{buildroot}%{_datadir}/sdcompat/s6-linux-init/skel
install -Dm0644 dist/rsyslog.conf %{buildroot}%{_datadir}/sdcompat/rsyslog.conf
install -Dm0755 dist/s6-linux-init/init-selinux %{buildroot}%{_sbindir}/init-selinux
install -Dm0644 dist/selinux/sdcompat.cil %{buildroot}%{_datadir}/sdcompat/selinux/sdcompat.cil
install -Dm0755 dist/kernel-install/90-sdcompat.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-sdcompat.install
install -d %{buildroot}%{_sharedstatedir}/sdcompat %{buildroot}%{_localstatedir}/log/sdcompat

%post
# Keep the s6 tree in init_t (see dist/selinux/sdcompat.cil). The skarnet
# packages are relabelled too; they are usually installed before this one.
if [ -x /usr/sbin/semodule ]; then
    semodule -i %{_datadir}/sdcompat/selinux/sdcompat.cil || :
    restorecon $(rpm -ql execline s6 s6-rc s6-linux-init 2>/dev/null | grep '^/usr/') \
        %{_libexecdir}/sdcompat/* 2>/dev/null || :
fi

%postun
if [ "$1" -eq 0 ] && [ -x /usr/sbin/semodule ]; then
    semodule -r sdcompat 2>/dev/null || :
    restorecon $(rpm -ql execline s6 s6-rc s6-linux-init 2>/dev/null | grep '^/usr/') 2>/dev/null || :
fi

%files
%{_bindir}/sdcompat
%{_prefix}/lib/kernel/install.d/90-sdcompat.install
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
