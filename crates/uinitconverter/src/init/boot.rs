//! The boot entry: adding, removing and choosing the default.

use super::*;

/// The BLS entries init added (title "Fedora (s6 + uinitconverter)...").
pub(super) fn s6_entries() -> Vec<String> {
    list_dir(BLS_DIR, |n| n.ends_with(".conf"))
        .into_iter()
        .filter(|f| {
            read(f).is_some_and(|t| t.lines().any(|l| l.starts_with(&format!("title {TITLE}"))))
        })
        .collect()
}

pub(super) fn remove_grub_entry(ctx: &Ctx) {
    if Path::new(BLS_DIR).is_dir() {
        for f in s6_entries() {
            ctx.run(&["rm", "-f", &f]);
        }
    } else if in_path("grubby") {
        ctx.try_run(&["grubby", &format!("--remove-kernel=TITLE={TITLE}")]);
    }
}

/// The GRUB environment's saved_entry (GRUB_DEFAULT=saved), or nothing.
pub(super) fn saved_entry() -> String {
    output(&["grub2-editenv", "-", "list"])
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("saved_entry=").map(String::from))
        .unwrap_or_default()
}

/// If the default is an s6 entry, make it the same kernel's systemd entry
/// (the s6 entry's BLS id is the systemd one's with "~uinitconverter" appended).
pub(super) fn systemd_default(ctx: &Ctx) {
    let saved = saved_entry();
    if let Some(id) = saved.strip_suffix("~uinitconverter") {
        if exists(&format!("{BLS_DIR}/{id}.conf")) {
            ctx.run(&["grub2-editenv", "-", "set", &format!("saved_entry={id}")]);
        } else {
            ctx.run(&["grub2-editenv", "-", "unset", "saved_entry"]);
        }
    }
}

pub(super) fn add_grub_entry(ctx: &Ctx, s: &Settings) {
    if !s.grub {
        return;
    }
    say(&format!("Adding boot entry: {TITLE}"));
    let args = boot_args(s.enforcing);
    ctx.put(
        BOOT_OPTIONS,
        &format!("# Extra kernel options for the \"{TITLE}\" boot entries.\n{args}\n"),
        0o644,
    );
    let kernel = if ctx.dry && !in_path("grubby") {
        "/boot/vmlinuz-KVER".to_string()
    } else {
        output(&["grubby", "--default-kernel"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| die("grubby --default-kernel failed"))
    };
    let kver = kernel
        .strip_prefix("/boot/vmlinuz-")
        .unwrap_or(&kernel)
        .to_string();
    // The plugin builds the s6 initramfs (--boot-only-s6=yes) while this file
    // exists, for the kernels below and for every later one.
    if s.boot_only_s6 {
        ctx.put(
            BOOT_ONLY_S6_FILE,
            "# The s6 entries boot with the s6 initramfs (initconvert init --boot-only-s6=yes).\n",
            0o644,
        );
    } else if exists(BOOT_ONLY_S6_FILE) {
        ctx.run(&["rm", "-f", BOOT_ONLY_S6_FILE]);
    }
    // An old setup-fedora.sh copy in /etc would hide the packaged plugin.
    if exists(OLD_KI_PLUGIN) {
        println!("  removing {OLD_KI_PLUGIN} (the package's {KI_PLUGIN} is used)");
        ctx.backup(OLD_KI_PLUGIN);
        ctx.run(&["rm", "-f", OLD_KI_PLUGIN]);
    }
    if Path::new(BLS_DIR).is_dir() || ctx.dry {
        // The kernel-install plugin copies the BLS entry for each kernel,
        // now and on every future kernel update.
        remove_grub_entry(ctx);
        // Every installed kernel, as the plugin keeps them: the removal
        // above took all their entries, not only the default kernel's.
        let kernels: Vec<String> = list_dir("/lib/modules", |_| true)
            .into_iter()
            .filter_map(|d| d.rsplit('/').next().map(String::from))
            .filter(|k| exists(&format!("/boot/vmlinuz-{k}")))
            .collect();
        if kernels.is_empty() {
            ctx.run(&[KI_PLUGIN, "add", &kver]);
        }
        for k in &kernels {
            ctx.run(&[KI_PLUGIN, "add", k]);
        }
    } else {
        remove_grub_entry(ctx);
        #[rustfmt::skip]
        ctx.run(&[
            "grubby", &format!("--add-kernel={kernel}"), &format!("--initrd=/boot/initramfs-{kver}.img"),
            "--copy-default", &format!("--title={TITLE} {kver}"), &format!("--args={args}"),
        ]);
        warn("no BLS entries: future kernels will not get an s6 entry automatically");
        if s.boot_only_s6 {
            warn("no BLS entries: --boot-only-s6=yes is not applied (the entry uses Fedora's initramfs)");
        }
    }
    if s.boot_only_s6 {
        println!("  The s6 entries boot with the s6 initramfs (/boot/initramfs-KVER-s6.img): no systemd at all.");
    }
    set_boot_default(ctx, s, &kver);
}

/// Make KVER's s6 entry (--default-s6) or the systemd entry the default.
/// The kernel-install plugin reads the same file and keeps the s6 entry the
/// default when a kernel is added or removed.
pub(super) fn set_boot_default(ctx: &Ctx, s: &Settings, kver: &str) {
    if !s.default_s6 {
        if exists(BOOT_DEFAULT_FILE) {
            ctx.run(&["rm", "-f", BOOT_DEFAULT_FILE]);
        }
        systemd_default(ctx);
        println!("  The systemd entry is the default. Pick '{TITLE}' in the GRUB menu.");
        return;
    }
    ctx.put(
        BOOT_DEFAULT_FILE,
        &format!("# s6: the \"{TITLE}\" entry is the default boot entry (initconvert init --default-s6).\ns6\n"),
        0o644,
    );
    let suffix = format!("{kver}~uinitconverter.conf");
    let id = list_dir(BLS_DIR, |n| n.ends_with(&suffix))
        .last()
        .and_then(|f| f.rsplit('/').next())
        .map(|n| n.trim_end_matches(".conf").to_string());
    match id {
        Some(id) => ctx.run(&["grub2-editenv", "-", "set", &format!("saved_entry={id}")]),
        None if ctx.dry => ctx.run(&[
            "grub2-editenv",
            "-",
            "set",
            &format!("saved_entry=ID-{kver}~uinitconverter"),
        ]),
        None => {
            warn(&format!("no s6 entry for {kver}: the default is unchanged"));
            return;
        }
    }
    println!("  '{TITLE}' is the default; the systemd entry stays in the GRUB menu.");
}
