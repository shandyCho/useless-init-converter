Name:           s6-networking
# Version and release come from dist/skarnet.versions (build-skarnet-rpms.sh
# defines them); build with that script, not with rpmbuild directly.
Version:        %{skarnet_version}
Release:        %{skarnet_release}%{?dist}
Summary:        TCP super-server and network tools from skarnet.org
License:        ISC
URL:            https://skarnet.org/software/s6-networking/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel execline-devel s6-devel s6-dns-devel

%description
s6-networking holds s6-tcpserverd, the TCP super-server uinitconverter
runs for Accept=yes TCP sockets, and the other UCSPI network tools. It is
built without TLS (no s6-tlsserver, s6-tlsclient).

%package devel
Summary:        Development files for s6-networking
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, shared library links and pkg-config files for s6-networking.

%prep
%autosetup

%build
%set_build_flags
# skarnet_sysroot: build-skarnet-rpms.sh builds against the RPMs it just made,
# unpacked there, instead of what is installed.
./configure \
    --prefix=%{_prefix} \
    --bindir=%{_bindir} \
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
    --enable-pkgconfig
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

%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/*.pc

%changelog
* Mon Nov 02 2026 uinitconverter - 2.8.0.1-1
- Initial package, without the TLS tools
