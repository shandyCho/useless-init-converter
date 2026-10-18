Name:           s6
Version:        2.15.1.0
Release:        1%{?dist}
Summary:        skarnet.org's small and secure supervision software suite
License:        ISC
URL:            https://skarnet.org/software/s6/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel execline-devel

%description
s6 is a small suite of programs for UNIX, designed to allow process
supervision (a.k.a. service supervision), in the line of daemontools and runit,
as well as various operations on processes and daemons.

%package devel
Summary:        Development files for s6
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, shared library links and pkg-config files for s6.

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
%make_install

%files
%license COPYING
%doc AUTHORS NEWS README
%{_bindir}/*
%{_libdir}/lib*.so.*
%{_libexecdir}/%{name}/

%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/libs6 libs6auto.pc

%changelog
* Sun Oct 18 2026 sdcompat - 2.15.1.0-1
- Initial package
