# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
# This one holds the uinitconverter command (with `uinitconverter init`, the boot
# setup) and the files it sets up from, and pulls in the tools as their own
# packages.
Name:           uinitconverter
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
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
# uinitconverter init: SELinux file labels and the boot entry.
Requires:       policycoreutils-python-utils
Requires:       grubby
# The tools, each its own RPM from this source.
Requires:       uinitconverter-gen = %{version}-%{release}
Requires:       uinitconverter-runner = %{version}-%{release}
Requires:       uinitconverter-timers = %{version}-%{release}
Requires:       uinitconverterd = %{version}-%{release}
Requires(post): policycoreutils
Requires(postun): policycoreutils
Recommends:     rsyslog
Recommends:     earlyoom
# The s6 command; uinitconverter init writes /etc/s6.conf for it.
Recommends:     s6-frontend
# The s6 initramfs (uinitconverter init --boot-only-s6=yes) loads drivers with it.
Recommends:     mdevd
# Separate projects (standalone/), each its own package; uinitconverter works
# without them. uinitconverter-journal-relay is needed for JOURNAL=no
# (uinitconverter init --journal=no).
Recommends:     uinitconverter-fdstore
Recommends:     uinitconverter-notify
Recommends:     uinitconverter-linkwatch
Suggests:       uinitconverter-journal-relay

%description
Runs Fedora's systemd units with s6 as the init system. This package holds
the uinitconverter command, which passes each subcommand to the tool that handles
it, and the boot setup: `sudo uinitconverter init` adds a GRUB entry that boots
with s6, from this package's configuration, hand-written s6-rc services,
s6-linux-init skeleton, SELinux policy module and kernel-install plugin. The tools come in their own packages: uinitconverter-gen, uinitconverter-runner,
uinitconverter-timers and uinitconverterd.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
cargo build --release --offline -p uinitconverter

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter -p uinitconverter-version

%install
install -Dm0755 target/release/uinitconverter %{buildroot}%{_libexecdir}/uinitconverter/uinitconverter
install -Dm0755 dist/early %{buildroot}%{_libexecdir}/uinitconverter/early
install -Dm0755 dist/initramfs/mkinitramfs %{buildroot}%{_libexecdir}/uinitconverter/mkinitramfs
install -d %{buildroot}%{_bindir}
ln -s ../libexec/uinitconverter/uinitconverter %{buildroot}%{_bindir}/uinitconverter
install -Dm0644 dist/uinitconverter.conf %{buildroot}%{_sysconfdir}/uinitconverter/uinitconverter.conf
install -d %{buildroot}%{_sysconfdir}/uinitconverter
cp -a dist/s6 %{buildroot}%{_sysconfdir}/uinitconverter/s6
install -d %{buildroot}%{_datadir}/uinitconverter/s6-linux-init
cp -a dist/s6-linux-init/skel %{buildroot}%{_datadir}/uinitconverter/s6-linux-init/skel
install -Dm0644 dist/rsyslog.conf %{buildroot}%{_datadir}/uinitconverter/rsyslog.conf
install -Dm0644 dist/s6.conf %{buildroot}%{_datadir}/uinitconverter/s6.conf
cp -a dist/initramfs/root %{buildroot}%{_datadir}/uinitconverter/initramfs
install -Dm0755 dist/s6-linux-init/init-selinux %{buildroot}%{_sbindir}/init-selinux
install -Dm0644 dist/selinux/uinitconverter.cil %{buildroot}%{_datadir}/uinitconverter/selinux/uinitconverter.cil
install -Dm0755 dist/kernel-install/90-uinitconverter.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-uinitconverter.install
install -d %{buildroot}%{_localstatedir}/log/uinitconverter

%post
# Keep the s6 tree in init_t (see dist/selinux/uinitconverter.cil). The skarnet
# packages and the uinitconverter tools are relabelled too; they are installed
# before this one, and later updates get the labels from the loaded policy.
if [ -x /usr/sbin/semodule ]; then
    semodule -i %{_datadir}/uinitconverter/selinux/uinitconverter.cil || :
    rpm -ql execline s6 s6-rc s6-linux-init 2>/dev/null | grep '^/usr/' |
        xargs -r -d '\n' restorecon 2>/dev/null || :
    restorecon %{_libexecdir}/uinitconverter/* 2>/dev/null || :
    [ -d /var/lib/uinitconverter ] && restorecon -R /var/lib/uinitconverter || :
    [ -d /var/lib/s6-rc ] && restorecon -R /var/lib/s6-rc || :
    [ -p /run/service/s6-svscan-log/fifo ] && restorecon /run/service/s6-svscan-log/fifo || :
    restorecon /run/service/*/event 2>/dev/null || :
fi

%posttrans
# The s6 initramfs (uinitconverter init --boot-only-s6=yes) holds this package's
# initramfs scripts: rebuild the images there are, so an update reaches the
# next boot and not only the next kernel. The plugin falls back to Fedora's
# image if a build fails.
if [ -e /etc/uinitconverter/boot-only-s6 ]; then
    for img in /boot/initramfs-*-s6.img; do
        [ -e "$img" ] || continue
        kver=${img#/boot/initramfs-}
        kver=${kver%%-s6.img}
        [ -d "/lib/modules/$kver" ] || continue
        %{_prefix}/lib/kernel/install.d/90-uinitconverter.install add "$kver" || :
    done
fi

%postun
if [ "$1" -eq 0 ] && [ -x /usr/sbin/semodule ]; then
    semodule -r uinitconverter 2>/dev/null || :
    rpm -ql execline s6 s6-rc s6-linux-init 2>/dev/null | grep '^/usr/' |
        xargs -r -d '\n' restorecon 2>/dev/null || :
    [ -d /var/lib/uinitconverter ] && restorecon -R /var/lib/uinitconverter || :
    [ -d /var/lib/s6-rc ] && restorecon -R /var/lib/s6-rc || :
fi

%files
%{_bindir}/uinitconverter
%{_prefix}/lib/kernel/install.d/90-uinitconverter.install
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverter
%{_libexecdir}/uinitconverter/early
%{_libexecdir}/uinitconverter/mkinitramfs
%{_sbindir}/init-selinux
%{_datadir}/uinitconverter/
%dir %{_sysconfdir}/uinitconverter
%config(noreplace) %{_sysconfdir}/uinitconverter/uinitconverter.conf
%config(noreplace) %{_sysconfdir}/uinitconverter/s6/
%dir %{_localstatedir}/log/uinitconverter

%changelog
* Tue Oct 27 2026 uinitconverter developers
- uinitconverter init replaces dist/setup-fedora.sh; ships s6.conf for it
- Requires policycoreutils-python-utils and grubby (used by uinitconverter init)

* Wed Oct 21 2026 uinitconverter developers
- The tools moved to their own packages: uinitconverter-gen, uinitconverter-runner,
  uinitconverter-timers, uinitconverterd

* Sun Oct 18 2026 uinitconverter developers
- Initial package
