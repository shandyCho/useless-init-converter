Name:           s6-frontend
# Version and release come from dist/skarnet.versions (build-skarnet-rpms.sh
# defines them); build with that script, not with rpmbuild directly.
Version:        %{skarnet_version}
Release:        %{skarnet_release}%{?dist}
Summary:        The s6 command: one frontend to s6, s6-rc and s6-linux-init
License:        ISC
URL:            https://skarnet.org/software/s6-frontend/
Source0:        https://skarnet.org/software/%{name}/%{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  skalibs-devel execline-devel s6-devel s6-rc-devel s6-linux-init-devel
# "column" for aligned output (--enable-util-linux).
Requires:       util-linux

%description
s6-frontend provides the s6 command, which takes intuitively named
subcommands (s6 live status, s6 process restart, ...) and runs the matching
s6, s6-rc and s6-linux-init programs with the right directories.

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
    --sysconfdir=%{_sysconfdir} \
    --with-sysdeps=%{?skarnet_sysroot}%{_libdir}/skalibs/sysdeps \
    %{?skarnet_sysroot:--with-include=%{skarnet_sysroot}%{_includedir} --with-dynlib=%{skarnet_sysroot}%{_libdir} --with-lib=%{skarnet_sysroot}%{_libdir}} \
    --disable-allstatic \
    --disable-rpath \
    --enable-util-linux
%make_build

%install
# Not %make_install: it sets INSTALL=/usr/bin/install, which lacks the -l
# (symlink) option of the bundled tools/install.sh.
%{__make} install DESTDIR=%{buildroot}
# Headers of the internal helper library; nothing links against it.
rm -rf %{buildroot}%{_includedir} %{buildroot}%{_libdir}/*.a

%files
%license COPYING
%doc AUTHORS NEWS README doc/*.html
%{_bindir}/s6
%{_libexecdir}/%{name}/

%changelog
* Tue Oct 20 2026 sdcompat - 0.1.0.0-1
- Initial package
