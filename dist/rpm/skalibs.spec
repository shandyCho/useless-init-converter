Name:           skalibs
Version:        2.15.1.0
Release:        1%{?dist}
Summary:        General-purpose C library used by skarnet.org software
License:        ISC
URL:            https://skarnet.org/software/skalibs/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make

%description
skalibs is a package centralizing the free software / open source C
development files used for building all software at skarnet.org: execline,
s6, s6-rc, s6-linux-init and others.

%package devel
Summary:        Development files for skalibs
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, the libskarnet.so link, the pkg-config file and the sysdeps
directory needed to build skarnet.org software.

%prep
%autosetup

%build
%set_build_flags
./configure \
    --prefix=%{_prefix} \
    --dynlibdir=%{_libdir} \
    --libdir=%{_libdir} \
    --includedir=%{_includedir} \
    --pkgconfdir=%{_libdir}/pkgconfig \
    --sysdepdir=%{_libdir}/skalibs/sysdeps \
    --disable-static \
    --disable-rpath \
    --enable-pkgconfig
%make_build

%install
%make_install

%files
%license COPYING
%doc AUTHORS NEWS README
%{_libdir}/libskarnet.so.*

%files devel
%{_includedir}/skalibs/
%{_libdir}/libskarnet.so
%{_libdir}/pkgconfig/libskarnet.pc
%{_libdir}/skalibs/

%changelog
* Sun Oct 18 2026 sdcompat - 2.15.1.0-1
- Initial package
