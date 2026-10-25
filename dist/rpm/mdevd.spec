Name:           mdevd
# Version and release come from dist/skarnet.versions (build-skarnet-rpms.sh
# defines them); build with that script, not with rpmbuild directly.
Version:        %{skarnet_version}
Release:        %{skarnet_release}%{?dist}
Summary:        A uevent manager, a daemon version of mdev
License:        ISC
URL:            https://skarnet.org/software/mdevd/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel

%description
mdevd is a small daemon managing kernel hotplug events, similarly to udevd,
with the configuration file format of busybox's mdev. sdcompat's s6
initramfs (setup-fedora.sh --boot-only-s6) uses it to load drivers.

%prep
%autosetup

%build
%set_build_flags
# skarnet_sysroot: build-skarnet-rpms.sh builds against the RPMs it just made,
# unpacked there, instead of what is installed.
./configure \
    --prefix=%{_prefix} \
    --bindir=%{_bindir} \
    --libexecdir=%{_libexecdir}/%{name} \
    --dynlibdir=%{_libdir} \
    --libdir=%{_libdir} \
    --sysconfdir=%{_sysconfdir} \
    --with-sysdeps=%{?skarnet_sysroot}%{_libdir}/skalibs/sysdeps \
    %{?skarnet_sysroot:--with-include=%{skarnet_sysroot}%{_includedir} --with-dynlib=%{skarnet_sysroot}%{_libdir} --with-lib=%{skarnet_sysroot}%{_libdir}} \
    --enable-shared \
    --disable-static \
    --disable-allstatic \
    --disable-rpath
%make_build

%install
# Not %make_install: it sets INSTALL=/usr/bin/install, which lacks the -l
# (symlink) option of the bundled tools/install.sh.
%{__make} install DESTDIR=%{buildroot}
# Its only header is the build configuration; there is no library to use it.
rm -rf %{buildroot}%{_includedir}/mdevd

%files
%license COPYING
%doc AUTHORS NEWS README
%{_bindir}/mdevd
%{_bindir}/mdevd-coldplug

%changelog
* Sun Oct 25 2026 sdcompat - 0.1.8.2-1
- Initial package
