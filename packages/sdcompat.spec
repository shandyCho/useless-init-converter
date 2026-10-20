# One of the sdcompat RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under sdcompat-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
# This one holds the sdcompat command and the boot setup, and pulls in the
# tools as their own packages.
Name:           sdcompat
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{sdcompat_version}
Release:        %{sdcompat_release}%{?dist}
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
# The tools, each its own RPM from this source.
Requires:       sdcompat-gen = %{version}-%{release}
Requires:       sdcompat-runner = %{version}-%{release}
Requires:       sdcompat-timers = %{version}-%{release}
Requires:       sdcompatd = %{version}-%{release}
Requires(post): policycoreutils
Requires(postun): policycoreutils
Recommends:     s6-linux-init
Recommends:     rsyslog
Recommends:     earlyoom
# The s6 command; setup-fedora.sh writes /etc/s6.conf for it.
Recommends:     s6-frontend
# Separate projects (standalone/), each its own package; sdcompat works
# without them. sdcompat-journal-relay is needed for JOURNAL=no.
Recommends:     sdcompat-fdstore
Recommends:     sdcompat-notify
Suggests:       sdcompat-journal-relay

%description
Runs Fedora's systemd units with s6 as the init system. This package holds
the sdcompat command, which passes each subcommand to the tool that handles
it, and the boot setup: configuration, hand-written s6-rc services, the
s6-linux-init skeleton, the SELinux policy module and the kernel-install
plugin. The tools come in their own packages: sdcompat-gen, sdcompat-runner,
sdcompat-timers and sdcompatd.

%prep
%autosetup -n sdcompat-%{version}

%build
cargo build --release --offline -p sdcompat

%check
cargo test --release --offline -p sdcompat

%install
install -Dm0755 target/release/sdcompat %{buildroot}%{_libexecdir}/sdcompat/sdcompat
install -Dm0755 dist/early %{buildroot}%{_libexecdir}/sdcompat/early
install -d %{buildroot}%{_bindir}
ln -s ../libexec/sdcompat/sdcompat %{buildroot}%{_bindir}/sdcompat
install -Dm0644 dist/sdcompat.conf %{buildroot}%{_sysconfdir}/sdcompat/sdcompat.conf
install -d %{buildroot}%{_sysconfdir}/sdcompat
cp -a dist/s6 %{buildroot}%{_sysconfdir}/sdcompat/s6
install -d %{buildroot}%{_datadir}/sdcompat/s6-linux-init
cp -a dist/s6-linux-init/skel %{buildroot}%{_datadir}/sdcompat/s6-linux-init/skel
install -Dm0644 dist/rsyslog.conf %{buildroot}%{_datadir}/sdcompat/rsyslog.conf
install -Dm0755 dist/s6-linux-init/init-selinux %{buildroot}%{_sbindir}/init-selinux
install -Dm0644 dist/selinux/sdcompat.cil %{buildroot}%{_datadir}/sdcompat/selinux/sdcompat.cil
install -Dm0755 dist/kernel-install/90-sdcompat.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-sdcompat.install
install -d %{buildroot}%{_localstatedir}/log/sdcompat

%post
# Keep the s6 tree in init_t (see dist/selinux/sdcompat.cil). The skarnet
# packages and the sdcompat tools are relabelled too; they are installed
# before this one, and later updates get the labels from the loaded policy.
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
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat
%{_libexecdir}/sdcompat/early
%{_sbindir}/init-selinux
%{_datadir}/sdcompat/
%dir %{_sysconfdir}/sdcompat
%config(noreplace) %{_sysconfdir}/sdcompat/sdcompat.conf
%config(noreplace) %{_sysconfdir}/sdcompat/s6/
%dir %{_localstatedir}/log/sdcompat

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- The tools moved to their own packages: sdcompat-gen, sdcompat-runner,
  sdcompat-timers, sdcompatd

* Sun Oct 18 2026 sdcompat developers - 0.1.0-1
- Initial package
