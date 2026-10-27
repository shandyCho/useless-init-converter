Name:           s6-linux-init
# Version and release come from dist/skarnet.versions (build-skarnet-rpms.sh
# defines them); build with that script, not with rpmbuild directly.
Version:        %{skarnet_version}
Release:        %{skarnet_release}%{?dist}
Summary:        Tools for running an s6-based init system on Linux
License:        ISC
URL:            https://skarnet.org/software/s6-linux-init/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel execline-devel s6-devel

%description
s6-linux-init is a set of minimalistic tools used to create an s6-based init
system, including a /sbin/init binary, on a Linux kernel.

%package devel
Summary:        Development files for s6-linux-init
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, shared library links and pkg-config files for s6-linux-init.

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
    --includedir=%{_includedir} \
    --pkgconfdir=%{_libdir}/pkgconfig \
    --with-sysdeps=%{?skarnet_sysroot}%{_libdir}/skalibs/sysdeps \
    %{?skarnet_sysroot:--with-include=%{skarnet_sysroot}%{_includedir} --with-dynlib=%{skarnet_sysroot}%{_libdir} --with-lib=%{skarnet_sysroot}%{_libdir}} \
    --enable-shared \
    --disable-static \
    --disable-allstatic \
    --disable-rpath \
    --enable-pkgconfig \
    --skeldir=%{_sysconfdir}/s6-linux-init/skel
%make_build

%install
# Not %make_install: it sets INSTALL=/usr/bin/install, which lacks the -l
# (symlink) option of the bundled tools/install.sh.
%{__make} install DESTDIR=%{buildroot}

%files
%license COPYING
%doc AUTHORS NEWS README
%{_bindir}/*
%{_libdir}/lib*.so.*
%dir %{_sysconfdir}/s6-linux-init
%config(noreplace) %{_sysconfdir}/s6-linux-init/skel/

%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/*.pc

%changelog
* Sun Oct 18 2026 uinitconverter - 1.2.0.2-1
- Initial package
