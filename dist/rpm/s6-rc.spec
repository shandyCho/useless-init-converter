Name:           s6-rc
Version:        0.7.0.0
Release:        1%{?dist}
Summary:        Service manager for s6-based systems
License:        ISC
URL:            https://skarnet.org/software/s6-rc/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel execline-devel s6-devel

%description
s6-rc is a service manager for s6-based systems, i.e. a suite of programs
that can start and stop services, both long-running daemons and one-time
initialization scripts, in the proper order according to a dependency tree.

%package devel
Summary:        Development files for s6-rc
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, shared library links and pkg-config files for s6-rc.

%prep
%autosetup

%build
%set_build_flags
./configure \
    --prefix=%{_prefix} \
    --bindir=%{_bindir} \
    --libexecdir=%{_libexecdir}/%{name} \
    --dynlibdir=%{_libdir} \
    --libdir=%{_libdir} \
    --includedir=%{_includedir} \
    --pkgconfdir=%{_libdir}/pkgconfig \
    --with-sysdeps=%{_libdir}/skalibs/sysdeps \
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
%{_libexecdir}/%{name}/

%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/libs6rc.pc
%{_libdir}/pkgconfig/libs6rcrepo.pc

%changelog
* Sun Oct 18 2026 sdcompat - 0.7.0.0-1
- Initial package
