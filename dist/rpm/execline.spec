Name:           execline
Version:        2.9.9.2
Release:        1%{?dist}
Summary:        Non-interactive scripting language
License:        ISC
URL:            https://skarnet.org/software/execline/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel

%description
execline is a very light non-interactive scripting language, similar to a
shell. Simple shell scripts can be easily rewritten in the execline language,
improving performance and memory usage. s6, s6-rc and s6-linux-init use it.

%package devel
Summary:        Development files for execline
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
Headers, shared library links and pkg-config files for execline.

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


%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/libexecline.pc

%changelog
* Sun Oct 18 2026 sdcompat - 2.9.9.2-1
- Initial package
