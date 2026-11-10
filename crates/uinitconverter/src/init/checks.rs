//! Checks before anything changes: the system, the programs, versions, LUKS.

use super::*;

pub(super) fn check_system(ctx: &Ctx) {
    if !ctx.dry && unsafe { libc::geteuid() } != 0 {
        die("run as root (or use --dry-run)");
    }
    let id = read("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            l.strip_prefix("ID=")
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    if id != "fedora" {
        if ctx.dry && std::env::var("UINITCONVERTER_INIT_ANY_OS").as_deref() == Ok("1") {
            warn(&format!("not Fedora ({id}); dry run only"));
        } else {
            die(&format!("initconvert init is for Fedora (found ID={id})"));
        }
    }
    // What the steps copy or run; the configure step reads the two
    // configuration files, so a real run needs them.
    let packaged = [
        format!("{SHARE}/s6-linux-init/skel"),
        format!("{SHARE}/selinux/uinitconverter.cil"),
        format!("{SHARE}/s6.conf"),
        format!("{SHARE}/rsyslog.conf"),
        format!("{SHARE}/syslog-ng.conf"),
        format!("{LIBEXEC}/initconvert-gen"),
    ];
    if let Some(p) = packaged.iter().find(|p| !exists(p)) {
        let msg = format!("{p} is missing: install the packages first (sudo dnf install ./*.rpm with the RPMs from {RELEASES})");
        if ctx.dry {
            warn(&msg);
        } else {
            die(&msg);
        }
    }
}

/// The programs this run needs, by package. Missing ones end init (a dry
/// run only warns).
pub(super) fn check_programs(ctx: &Ctx, s: &Settings) {
    let mut need: Vec<(bool, &str)> = vec![
        (in_path("s6-linux-init-maker"), "s6-linux-init"),
        (in_path("semanage"), "policycoreutils-python-utils"),
    ];
    if s.grub {
        need.push((in_path("grubby"), "grubby"));
    }
    if s.grub && s.default_s6 {
        need.push((in_path("grub2-editenv"), "grub2-tools"));
    }
    if !s.journal {
        need.push((
            exists(&format!("{LIBEXEC}/initconvert-journal-relay")),
            "uinitconverter-journal-relay",
        ));
        if s.syslog == "syslog-ng" {
            need.push((in_path("syslog-ng"), "syslog-ng"));
        } else {
            need.push((in_path("rsyslogd"), "rsyslog"));
        }
    }
    if s.broker {
        need.push((in_path("dbus-broker-launch"), "dbus-broker"));
    }
    if s.grub && s.boot_only_s6 {
        need.push((in_path("mdevd"), "mdevd"));
    }
    let missing: Vec<&str> = need.iter().filter(|(ok, _)| !ok).map(|(_, p)| *p).collect();
    if !missing.is_empty() {
        let mut msg = format!(
            "missing packages for these settings: sudo dnf install {}",
            missing.join(" ")
        );
        if missing.iter().any(|p| SKARNET_PACKAGES.contains(p)) {
            msg += "; build s6-linux-init and mdevd with dist/rpm/build-skarnet-rpms.sh";
        }
        if missing.iter().any(|p| RELEASE_PACKAGES.contains(p)) {
            msg += &format!("; uinitconverter-journal-relay is an RPM in {RELEASES}");
        }
        if ctx.dry {
            warn(&msg);
        } else {
            die(&msg);
        }
    }
    if !rpm_installed("gdm") || !rpm_installed("gnome-shell") {
        warn("gdm/gnome-shell are not installed. For a full desktop run:");
        warn("  dnf group install workstation-product-environment");
    }
}

/// Show which builds are installed; the core tools must be from one build.
pub(super) fn check_versions(ctx: &Ctx) {
    if ctx.dry {
        println!("  + initconvert --version");
        return;
    }
    let (lines, ok) = crate::version_report();
    for l in lines {
        println!("  {}", l.trim_start());
    }
    if !ok {
        warn(&format!(
            "the uinitconverter packages are not all from one build (see above): sudo dnf upgrade {CORE_PACKAGES}"
        ));
    }
}

/// What the kernel command line asks an initramfs to assemble and the s6
/// initramfs can't: it has no LVM or MD tools.
pub(super) fn unassembled_root(cmdline: &str) -> Vec<&str> {
    cmdline
        .split_whitespace()
        .filter(|w| {
            ["rd.lvm.lv=", "rd.lvm.vg=", "rd.md.uuid="]
                .iter()
                .any(|p| w.starts_with(p))
        })
        .collect()
}

/// Why the s6 initramfs can't boot this system (--boot-only-s6=auto keeps
/// Fedora's then), or None: it opens LUKS with a typed passphrase and mounts
/// a root on a disk or partition; it has no LVM, MD or multipath tools, no
/// network, mounts no /usr of its own, doesn't resume from hibernation and
/// has no token plugins (TPM2, FIDO2, PKCS#11).
pub(super) fn s6_initramfs_blocker() -> Option<String> {
    if !in_path("mdevd") {
        return Some("mdevd is not installed".into());
    }
    let cmdline = read("/proc/cmdline").unwrap_or_default();
    let found = unassembled_root(&cmdline);
    if !found.is_empty() {
        return Some(format!("the kernel command line asks for {}", found.join(" ")));
    }
    if cmdline.split_whitespace().any(|w| w.starts_with("resume=")) {
        return Some("resume= (hibernation) is on the kernel command line".into());
    }
    let crypttab = read("/etc/crypttab").unwrap_or_default();
    if let Some(t) = ["tpm2-device", "fido2-device", "pkcs11-uri"]
        .iter()
        .find(|t| crypttab.contains(*t))
    {
        return Some(format!("/etc/crypttab unlocks a volume with {t}"));
    }
    if output(&["findmnt", "-n", "-o", "SOURCE", "/usr"]).is_some_and(|s| !s.trim().is_empty()) {
        return Some("/usr is a filesystem of its own".into());
    }
    let src = output(&["findmnt", "-n", "-o", "SOURCE,FSTYPE", "/"]).unwrap_or_default();
    let mut w = src.split_whitespace();
    let dev = w.next().unwrap_or("");
    let fstype = w.next().unwrap_or("");
    if fstype.starts_with("nfs") || fstype == "cifs" || fstype == "smb3" {
        return Some(format!("the root is on the network ({fstype})"));
    }
    // A btrfs subvolume's source is DEV[/subvol].
    let dev = dev.split('[').next().unwrap_or(dev);
    if !dev.starts_with("/dev/") {
        return Some(format!("the root's source is {dev}"));
    }
    root_stack_blocker(&output(&["lsblk", "-s", "-n", "-o", "TYPE", dev]).unwrap_or_default())
}

/// What in the root's device stack (lsblk -s TYPE, the root first, then
/// what it sits on) the s6 initramfs can't assemble.
pub(super) fn root_stack_blocker(types: &str) -> Option<String> {
    types
        .split_whitespace()
        .find(|t| *t == "lvm" || t.starts_with("raid") || *t == "md" || *t == "mpath" || *t == "loop")
        .map(|t| format!("the root is on {t}"))
}

/// A root on LVM or MD doesn't boot with --boot-only-s6=yes: the s6 entry would
/// stop at the initramfs's rescue shell. Fedora's initramfs handles them.
pub(super) fn check_root_stack(ctx: &Ctx, s: &Settings) {
    if !s.boot_only_s6 {
        return;
    }
    let cmdline = read("/proc/cmdline").unwrap_or_default();
    let found = unassembled_root(&cmdline);
    if !found.is_empty() {
        warn(&format!(
            "the s6 initramfs can't assemble this root: {}",
            found.join(" ")
        ));
        warn("  It has no LVM or MD tools, so the s6 entry would stop at a rescue shell.");
        warn("  Use --boot-only-s6=no, which keeps Fedora's initramfs.");
        ctx.confirm("Continue anyway?");
    }
}

/// The /etc/crypttab entries, each with whether the initramfs unlocks it.
pub(super) fn luks_volumes(crypttab: &str, cmdline: &str) -> Vec<(String, bool)> {
    let cmdline = format!(" {} ", cmdline.trim());
    crypttab
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let name = f.next()?;
            if name.starts_with('#') {
                return None;
            }
            let dev = f.next().unwrap_or("");
            let uuid = dev.strip_prefix("UUID=").unwrap_or(dev);
            let early = cmdline.contains(&format!(" rd.luks.uuid={uuid} "))
                || cmdline.contains(&format!(" rd.luks.uuid=luks-{uuid} "))
                || cmdline.contains(&format!(" rd.luks.name={uuid}="));
            Some((name.to_string(), early))
        })
        .collect()
}

/// LUKS: volumes the initramfs unlocks (rd.luks.uuid=) are open before s6
/// starts. Anything else in /etc/crypttab is opened by systemd-cryptsetup@
/// units on a systemd boot, which do not exist under s6.
pub(super) fn check_luks(ctx: &Ctx) {
    let Some(crypttab) = read("/etc/crypttab").filter(|t| !t.trim().is_empty()) else {
        return;
    };
    say("Checking encrypted volumes (/etc/crypttab)");
    let mut late = Vec::new();
    for (name, early) in luks_volumes(&crypttab, &read("/proc/cmdline").unwrap_or_default()) {
        if early {
            println!("  {name}: unlocked by the initramfs (fine)");
        } else {
            late.push(name);
        }
    }
    if !late.is_empty() {
        warn(&format!(
            "not unlocked by the initramfs: {}",
            late.join(" ")
        ));
        warn("  Under s6 nothing opens these (systemd-cryptsetup@ units are systemd-only),");
        warn("  so filesystems on them will not be mounted. Add them to the initramfs with");
        warn("  rd.luks.uuid= on the kernel command line, or leave them unused for this test.");
        ctx.confirm("Continue anyway?");
    }
    println!("  The LUKS passphrase prompt will be plain text (the s6 entry disables plymouth).");
}
