# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
# This one holds the initconvert command (with `initconvert init`, the boot
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
# dbus-uuidgen: /etc/machine-id when it is missing (early mount-api).
Requires:       dbus-tools
# initconvert init: SELinux file labels and the boot entry.
Requires:       policycoreutils-python-utils
Requires:       grubby
# The tools, each its own RPM from this source.
Requires:       uinitconverter-gen = %{version}-%{release}
Requires:       uinitconverter-runner = %{version}-%{release}
Requires:       uinitconverter-timers = %{version}-%{release}
Requires:       uinitconverterd = %{version}-%{release}
Requires(post): policycoreutils
Requires(postun): policycoreutils
# JOURNAL=no: the syslog daemon of the s6 boot (SYSLOG=, initconvert init --syslog=).
# The s6 "syslog" service (JOURNAL=no, the default).
Requires:       (rsyslog or syslog-ng)
Recommends:     earlyoom
# The console keymap and font (early vconsole).
Recommends:     kbd
# The s6 command; initconvert init writes /etc/s6.conf for it.
Recommends:     s6-frontend
# The s6 initramfs (initconvert init --boot-only-s6=yes) loads drivers with it.
Recommends:     mdevd
# Separate projects (standalone/), each its own package; uinitconverter works
# without them. uinitconverter-journal-relay is needed for JOURNAL=no
# (initconvert init --journal=no).
Recommends:     uinitconverter-fdstore
Recommends:     uinitconverter-notify
Recommends:     uinitconverter-linkwatch
Suggests:       uinitconverter-journal-relay

%description
Runs Fedora's systemd units with s6 as the init system. This package holds
the uinitconverter command, which passes each subcommand to the tool that handles
it, and the boot setup: `sudo initconvert init` adds a GRUB entry that boots
with s6, from this package's configuration, hand-written s6-rc services,
s6-linux-init skeleton, SELinux policy module and kernel-install plugin. The tools come in their own packages: uinitconverter-gen, uinitconverter-runner,
uinitconverter-timers and uinitconverterd.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
%if %{defined uinitconverter_target}
# build-rpms.sh --shared-target (test builds only): every package builds
# from one copy of the source at a fixed path into one target directory, so
# cargo reuses what an earlier package or build compiled (it rebuilds a
# crate whose source path changed). %%install reads target/ as usual.
rm -rf %{uinitconverter_target}/src
cp -a . %{uinitconverter_target}/src
ln -sfn %{uinitconverter_target}/target target
export CARGO_TARGET_DIR=%{uinitconverter_target}/target
cd %{uinitconverter_target}/src
%endif
cargo build --release --offline -p uinitconverter

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter -p uinitconverter-version

%install
install -Dm0755 target/release/initconvert %{buildroot}%{_libexecdir}/uinitconverter/initconvert
# The s6 services' early-boot steps are initconvert's (argv0 "early").
ln -s initconvert %{buildroot}%{_libexecdir}/uinitconverter/early
# The s6 initramfs builder: initconvert by argv0 (rule 6).
ln -s initconvert %{buildroot}%{_libexecdir}/uinitconverter/mkinitramfs
install -d %{buildroot}%{_bindir}
ln -s ../libexec/uinitconverter/initconvert %{buildroot}%{_bindir}/initconvert
install -Dm0644 dist/uinitconverter.conf %{buildroot}%{_sysconfdir}/uinitconverter/uinitconverter.conf
install -d %{buildroot}%{_sysconfdir}/uinitconverter
cp -a dist/s6 %{buildroot}%{_sysconfdir}/uinitconverter/s6
install -d %{buildroot}%{_datadir}/uinitconverter/s6-linux-init
cp -a dist/s6-linux-init/skel %{buildroot}%{_datadir}/uinitconverter/s6-linux-init/skel
install -Dm0644 dist/rsyslog.conf %{buildroot}%{_datadir}/uinitconverter/rsyslog.conf
install -Dm0644 dist/syslog-ng.conf %{buildroot}%{_datadir}/uinitconverter/syslog-ng.conf
install -Dm0644 dist/s6.conf %{buildroot}%{_datadir}/uinitconverter/s6.conf
cp -a dist/initramfs/root %{buildroot}%{_datadir}/uinitconverter/initramfs
# PID 1 before s6-linux-init: initconvert by argv0 (rule 6).
install -d %{buildroot}%{_sbindir}
ln -s ../libexec/uinitconverter/initconvert %{buildroot}%{_sbindir}/init-selinux
install -Dm0644 dist/selinux/uinitconverter.cil %{buildroot}%{_datadir}/uinitconverter/selinux/uinitconverter.cil
# The kernel-install plugin: initconvert by argv0 (rule 6).
install -d %{buildroot}%{_prefix}/lib/kernel/install.d
ln -s ../../../libexec/uinitconverter/initconvert %{buildroot}%{_prefix}/lib/kernel/install.d/90-uinitconverter.install
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
    [ -d /run/service/.s6-svscan ] && restorecon -R /run/service/.s6-svscan || :
    restorecon -R /run/service/*/supervise 2>/dev/null || :
    [ -p /run/service/s6-linux-init-shutdownd/fifo ] && restorecon /run/service/s6-linux-init-shutdownd/fifo || :
fi

# Defaults that changed (rule 2: RESOLVED=no, JOURNAL=no): an update keeps
# what an existing install runs, written as `initconvert init --KEY=yes`
# writes it; init then changes it as asked. A missing key means init hasn't
# run since the change: init writes KEY=no too, never removes the key
# (work_history part 231).
if [ "$1" -ge 2 ] && [ -f %{_sysconfdir}/uinitconverter/uinitconverter.conf ]; then
    conf=%{_sysconfdir}/uinitconverter/uinitconverter.conf
    for k in RESOLVED JOURNAL; do
        if ! grep -q "^[[:space:]]*$k=" "$conf"; then
            o=$(echo "$k" | tr 'A-Z' 'a-z')
            printf '\n# set by uinitconverter init --%s=yes\n%s=yes\n' "$o" "$k" >>"$conf" || :
        fi
    done
fi

%posttrans
# The s6 initramfs (initconvert init --boot-only-s6=yes) holds this package's
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
%{_bindir}/initconvert
%{_prefix}/lib/kernel/install.d/90-uinitconverter.install
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert
%{_libexecdir}/uinitconverter/early
%{_libexecdir}/uinitconverter/mkinitramfs
%{_sbindir}/init-selinux
%{_datadir}/uinitconverter/
%dir %{_sysconfdir}/uinitconverter
%config(noreplace) %{_sysconfdir}/uinitconverter/uinitconverter.conf
%config(noreplace) %{_sysconfdir}/uinitconverter/s6/
%dir %attr(0750,root,wheel) %{_localstatedir}/log/uinitconverter

%changelog
* Tue Nov 10 2026 uinitconverter developers
- No longer Suggests uinitconverter-user (removed; user@UID runs
  systemd --user)

* Mon Nov 09 2026 uinitconverter developers
- 0.3.0, see CHANGELOG-us.md

* Mon Nov 02 2026 uinitconverter developers
- 0.2.2, see CHANGELOG-us.md

* Mon Nov 02 2026 uinitconverter developers
- 0.2.1, see CHANGELOG-us.md

* Sun Nov 01 2026 uinitconverter developers
- 0.2.0, see CHANGELOG-us.md

* Sat Oct 31 2026 uinitconverter developers
- /var/log/uinitconverter is 0750 root:wheel
- s6 service coredump: systemd-coredump's listener, so core dumps are kept

* Wed Oct 28 2026 uinitconverter developers
- Ships syslog-ng.conf; recommends rsyslog or syslog-ng (SYSLOG=)

* Tue Oct 27 2026 uinitconverter developers
- uinitconverter init replaces dist/setup-fedora.sh; ships s6.conf for it
- Requires policycoreutils-python-utils and grubby (used by uinitconverter init)

* Wed Oct 21 2026 uinitconverter developers
- The tools moved to their own packages: uinitconverter-gen, uinitconverter-runner,
  uinitconverter-timers, uinitconverterd

* Sun Oct 18 2026 uinitconverter developers
- Initial package
