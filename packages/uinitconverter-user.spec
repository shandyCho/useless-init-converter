# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-user
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        The s6 user manager: user@UID.service without systemd --user
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz
# A link and a tmpfiles.d file: nothing is compiled here.
BuildArch:      noarch

# The user manager is initconvertd in user mode (the link points at it),
# with its own s6-svscan; the runner switches user@ to it.
Requires:       uinitconverterd = %{version}-%{release}
Requires:       uinitconverter-runner = %{version}-%{release}
Requires:       s6
# Its fallback (and USER_MANAGER=systemd) is systemd --user.
Requires:       systemd

%description
The s6 user manager: with USER_MANAGER=s6 in uinitconverter.conf, or
uinitconverter.user_manager=s6 on the kernel command line, user@UID.service
runs initconvert-user-manager instead of systemd --user. It serves
org.freedesktop.systemd1 on the user bus and runs the user's units under its
own s6-svscan; if it fails a user, it hands over to systemd --user for the
rest of the boot (the record is in /run/uinitconverter-user). Without this
package, user@ keeps systemd --user whatever USER_MANAGER says.

%prep
%autosetup -n uinitconverter-%{version}

%build

%install
install -d %{buildroot}%{_libexecdir}/uinitconverter
ln -s initconvertd %{buildroot}%{_libexecdir}/uinitconverter/initconvert-user-manager
install -Dm0644 dist/tmpfiles.d/uinitconverter-user.conf %{buildroot}%{_prefix}/lib/tmpfiles.d/uinitconverter-user.conf

%post
# The fallback record's directory, now as at each boot.
systemd-tmpfiles --create %{_prefix}/lib/tmpfiles.d/uinitconverter-user.conf >/dev/null 2>&1 || :

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-user-manager
%{_prefix}/lib/tmpfiles.d/uinitconverter-user.conf

%changelog
* Tue Nov 03 2026 uinitconverter developers
- New: the s6 user manager's entry point and fallback directory, out of
  uinitconverterd
