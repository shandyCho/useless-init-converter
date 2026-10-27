Name:           execline
# Version and release come from dist/skarnet.versions (build-skarnet-rpms.sh
# defines them); build with that script, not with rpmbuild directly.
Version:        %{skarnet_version}
Release:        %{skarnet_release}%{?dist}
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
    --enable-multicall
%make_build

%install
# Not %make_install: it sets INSTALL=/usr/bin/install, which lacks the -l
# (symlink) option of the bundled tools/install.sh.
%{__make} install DESTDIR=%{buildroot}
# One multicall binary, /usr/bin/execline, and a symlink per command: the
# SELinux module (dist/selinux/uinitconverter.cil) labels that one file, whatever
# commands a release has.
#
# bash owns /usr/bin/cd, umask and wait (POSIX wrappers around its builtins),
# so execline's go to %{_libexecdir}/%{name}, out of PATH. Only s6-linux-init's
# container mode (-C, not used by uinitconverter) calls wait.
mkdir -p %{buildroot}%{_libexecdir}/%{name}
for c in cd umask wait; do
    rm %{buildroot}%{_bindir}/$c
    ln -s %{_bindir}/execline %{buildroot}%{_libexecdir}/%{name}/$c
done

%files
%license COPYING
%doc AUTHORS NEWS README
%{_bindir}/*
%{_libexecdir}/%{name}/
%{_libdir}/lib*.so.*


%files devel
%{_includedir}/*/
%{_libdir}/lib*.so
%{_libdir}/pkgconfig/*.pc

%changelog
* Mon Oct 19 2026 uinitconverter - 2.9.9.2-2
- Do not ship /usr/bin/cd, umask, wait (owned by bash); wait moves to
  /usr/libexec/execline

* Sun Oct 18 2026 uinitconverter - 2.9.9.2-1
- Initial package
