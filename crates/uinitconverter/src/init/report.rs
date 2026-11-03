//! What `init` prints: the plan, the next steps, and the dry-run ending.

use super::*;

pub(super) fn plan(s: &Settings, backup: &str) -> String {
    let logs = if s.journal {
        "systemd-journald as an s6 service (journalctl works)".to_string()
    } else {
        format!(
            "{} + s6-log directories, no journal (--journal=no, --syslog={})",
            s.syslog, s.syslog
        )
    };
    let dns = if s.resolved {
        "systemd-resolved as on the systemd boot"
    } else {
        "NetworkManager without systemd-resolved (s6 boot only; --resolved=no)"
    };
    let bus = if s.broker {
        "dbus-broker as on the systemd boot (--bus=broker; dbus-daemon if it can't start)"
    } else {
        "dbus-daemon (--bus=daemon)"
    };
    let grub = if !s.grub {
        "  * GRUB:    unchanged (--grub=no)\n".to_string()
    } else {
        format!(
            "  * GRUB:    add \"{TITLE}\" with init=/usr/sbin/init-selinux, no splash, SELinux {},\n\
             \x20            and keep it for every new kernel (the package's kernel-install plugin);\n\
             \x20            default entry: {}\n\
             \x20 * initramfs: {}\n",
            if s.enforcing { "enforcing" } else { "permissive" },
            if s.default_s6 { format!("{TITLE} (--default-s6)") } else { "the systemd one".into() },
            if s.boot_only_s6 {
                "the s6 entry boots with uinitconverter's s6 initramfs (s6-svscan as its PID 1,\n\
                 \x20            mdevd, passphrase prompt; --boot-only-s6=yes), built for every kernel"
            } else {
                "Fedora's (systemd inside) for both entries"
            }
        )
    };
    format!(
        "\n\
         This will set up booting Fedora with s6-linux-init + s6-rc + uinitconverter:\n\
         \n\
         \x20 * write:   /etc/uinitconverter (settings, boot options), {S6LI_BASE}, /etc/s6.conf\n\
         \x20 * enable:  chronyd, NetworkManager, gdm; default target graphical.target\n\
         \x20 * SELinux: label {S6LI_BASE}/bin/init as init_exec_t; load the policy module\n\
         \x20            {SHARE}/selinux/uinitconverter.cil (s6 tools and uinitconverter as init_exec_t,\n\
         \x20            so services run in their own domains as under systemd)\n\
         \x20 * logs:    {logs}\n\
         \x20 * DNS:     {dns}\n\
         \x20 * D-Bus:   {bus}\n\
         {grub}\
         \n\
         Nothing about the normal systemd boot changes: systemd, dbus-broker, journald\n\
         and all your units stay installed{}.\n\
         Backups of overwritten files go to {backup}.\n",
        if s.default_s6 {
            ", and the systemd entry stays in the menu"
        } else {
            "; the new entry is not made the default"
        }
    )
}

pub(super) fn next_steps(s: &Settings) -> String {
    let reboot = if !s.grub {
        format!("Reboot and choose \"{TITLE}\" in the GRUB menu (if you added an entry yourself).")
    } else if s.default_s6 {
        format!("Reboot (\"{TITLE}\" is the default).")
    } else {
        format!("Reboot and choose \"{TITLE}\" in the GRUB menu.")
    };
    let selinux = if s.enforcing {
        "  4. SELinux: the entry boots enforcing. Check for denials with\n\
         \x20      ausearch -m AVC -ts boot\n"
    } else {
        "  4. SELinux: the entry boots permissive. Collect denials with\n\
         \x20      ausearch -m AVC -ts boot | audit2allow -M uinitconverter_local\n\
         \x20    review uinitconverter_local.te, load it with semodule -i, then run\n\
         \x20      sudo initconvert init --selinux=enforcing\n"
    };
    let only_s6 = if s.boot_only_s6 {
        "     With the s6 initramfs (--boot-only-s6=yes) an encrypted root's passphrase\n\
         \x20    prompt is plain text on the console. A failure opens a rescue shell (exit it to\n\
         \x20    try again); rd.break on the kernel command line gives a shell before the\n\
         \x20    switch. To boot one time with Fedora's initramfs, change the entry's\n\
         \x20    initrd line from initramfs-KVER-s6.img to initramfs-KVER.img (press e).\n"
    } else {
        ""
    };
    format!(
        "\n\
         Done. Next:\n\
         \x20 1. {reboot}\n\
         \x20 2. If stage 2 fails, log in on tty9 (Ctrl+Alt+F9) and look at:\n\
         \x20      /run/uncaught-logs/current        (s6 catch-all log)\n\
         \x20      s6-rc -a list                     (what is up)\n\
         \x20 3. Once booted:  systemctl status, systemctl list-units --failed,\n\
         \x20                  journalctl -b, journalctl -u systemd-logind, journalctl -u user@1000\n\
         {selinux}\
         \x20 5. If a new s6 database breaks booting, add this to the kernel command\n\
         \x20    line in GRUB (press e):\n\
         \x20      uinitconverter.db=previous    boot the database from before the last change\n\
         \x20      uinitconverter.bus=daemon     system bus with dbus-daemon for this boot\n\
         {only_s6}\
         \x20 6. To go back: boot the normal Fedora entry (hold Shift or press Esc for the\n\
         \x20    GRUB menu); make it the default again with\n\
         \x20      sudo initconvert init --default-systemd\n\
         \x20    To remove the s6 boot entry and what init wrote:\n\
         \x20      sudo initconvert init --uninstall\n"
    )
}

/// The end of a --dry-run: the command that would apply what was shown.
pub(super) fn dry_run_end(args: &[String]) -> String {
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--dry-run")
        .collect();
    let cmd = if rest.is_empty() {
        "sudo initconvert init".to_string()
    } else {
        format!("sudo initconvert init {}", rest.join(" "))
    };
    format!("\nDry run: nothing was changed. To apply this, run:\n  {cmd}\n")
}
