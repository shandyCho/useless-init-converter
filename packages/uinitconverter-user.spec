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
# A link, a script and a tmpfiles.d file: nothing is compiled here.
BuildArch:      noarch

# The user manager is initconvertd in user mode (the link points at it),
# with its own s6-svscan; the runner switches user@ to it.
Requires:       uinitconverterd = %{version}-%{release}
Requires:       uinitconverter-runner = %{version}-%{release}
# User timers (.timer units) run it.
Requires:       uinitconverter-timers = %{version}-%{release}
Requires:       s6
# Its fallback (and USER_MANAGER=systemd) is systemd --user.
Requires:       systemd

%description
The s6 user manager: user@UID.service runs initconvert-user-manager instead
of systemd --user. It is opt-in and not recommended: it reimplements
systemd --user and can't be kept in step with its changes, so a GNOME or
systemd update may break login. Installing it changes nothing until it is
turned on. On a normal systemd boot, `initconvert-user-setup on`
turns it on (and `off` back to systemd --user); it needs none of the s6 boot
packages. On the s6 boot, USER_MANAGER=s6 in uinitconverter.conf does. In
either case uinitconverter.user_manager=systemd on the kernel command line
runs systemd --user for one boot. It serves
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
install -Dm0755 dist/user/initconvert-user-setup %{buildroot}%{_bindir}/initconvert-user-setup

%post
# The fallback record's directory, now as at each boot.
systemd-tmpfiles --create %{_prefix}/lib/tmpfiles.d/uinitconverter-user.conf >/dev/null 2>&1 || :

%preun
# Removed, not upgraded: the drop-in `initconvert-user-setup on` wrote would
# point user@ at a missing program. USER_MANAGER= stays; without this package
# it changes nothing.
if [ "$1" -eq 0 ] && [ -e /etc/systemd/system/user@.service.d/90-uinitconverter.conf ]; then
    rm -f /etc/systemd/system/user@.service.d/90-uinitconverter.conf
    rmdir /etc/systemd/system/user@.service.d 2>/dev/null || :
    systemctl daemon-reload >/dev/null 2>&1 || :
fi

%files
%{_bindir}/initconvert-user-setup
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-user-manager
%{_prefix}/lib/tmpfiles.d/uinitconverter-user.conf

%changelog
* Mon Nov 09 2026 uinitconverter developers
- 0.3.0, see CHANGELOG-us.md

* Tue Nov 03 2026 uinitconverter developers
- initconvert-user-setup on|off|status: the s6 user manager on a systemd
  boot; Requires uinitconverter-timers (user timers); the drop-in goes when
  the package is removed
- New: the s6 user manager's entry point and fallback directory, out of
  uinitconverterd
