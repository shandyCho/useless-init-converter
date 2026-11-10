//! The kernel-install plugin (rule 6: was a shell script):
//! /usr/lib/kernel/install.d/90-uinitconverter.install is a link to
//! initconvert. It keeps a "Fedora (s6 + uinitconverter)" boot entry for
//! every installed kernel: a copy of the BLS entry 20-grub.install just
//! wrote, with the s6 options from /etc/uinitconverter/boot-options. With
//! /etc/uinitconverter/boot-only-s6 (initconvert init --boot-only-s6=yes) it
//! also builds the s6 initramfs (initramfs-VERSION-s6.img, PID 1
//! s6-svscan, no systemd) and boots the s6 entry with it; the systemd entry
//! keeps Fedora's.
//!   kernel-install add VERSION IMAGE  /  kernel-install remove VERSION
//! The UINITCONVERTER_* variables move every path, for tests.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use crate::early::{output, run};

const TITLE: &str = "Fedora (s6 + uinitconverter)";

struct Paths {
    entries: PathBuf,
    opts: PathBuf,
    default: PathBuf,
    editenv: String,
    only_s6: PathBuf,
    mkinitramfs: String,
    boot: PathBuf,
}

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

impl Paths {
    fn from_env() -> Paths {
        Paths {
            entries: env_or("UINITCONVERTER_BLS_DIR", "/boot/loader/entries").into(),
            opts: env_or(
                "UINITCONVERTER_BOOT_OPTIONS",
                "/etc/uinitconverter/boot-options",
            )
            .into(),
            // "s6" here (initconvert init --default-s6) keeps the s6 entry
            // the default.
            default: env_or(
                "UINITCONVERTER_BOOT_DEFAULT",
                "/etc/uinitconverter/boot-default",
            )
            .into(),
            editenv: env_or("UINITCONVERTER_GRUB_EDITENV", "grub2-editenv"),
            only_s6: env_or(
                "UINITCONVERTER_BOOT_ONLY_S6",
                "/etc/uinitconverter/boot-only-s6",
            )
            .into(),
            mkinitramfs: env_or(
                "UINITCONVERTER_MKINITRAMFS",
                "/usr/libexec/uinitconverter/mkinitramfs",
            ),
            boot: env_or("UINITCONVERTER_BOOT_DIR", "/boot").into(),
        }
    }
}

/// A file's text without # comments.
fn uncommented(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").to_string())
        .collect()
}

/// Whether PROG runs: a path, or a name in PATH.
fn runs(prog: &str) -> bool {
    if prog.contains('/') {
        return Path::new(prog).is_file();
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

fn s6_default(p: &Paths) -> bool {
    let v: String = uncommented(&fs::read_to_string(&p.default).unwrap_or_default())
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    v == "s6" && runs(&p.editenv)
}

/// VAR's value in the GRUB environment block.
fn envget(p: &Paths, var: &str) -> Option<String> {
    let out = output(&p.editenv, &["-", "list"])?;
    out.lines()
        .find_map(|l| l.strip_prefix(&format!("{var}=")))
        .map(str::to_string)
}

fn envset(p: &Paths, kv: &str) {
    run(&p.editenv, &["-", "set", kv]);
}

/// Compare as `ls -v` (GNU version sort, close enough for BLS ids): runs of
/// digits by value, everything else byte by byte; "~" sorts before anything,
/// the end of the name too.
fn vercmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    loop {
        match (a.get(i), b.get(j)) {
            (None, None) => return Ordering::Equal,
            (Some(b'~'), Some(b'~')) => {
                i += 1;
                j += 1;
            }
            (Some(b'~'), _) => return Ordering::Less,
            (_, Some(b'~')) => return Ordering::Greater,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let si = i;
                while a.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let sj = j;
                while b.get(j).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                }
                let na = std::str::from_utf8(&a[si..i])
                    .unwrap()
                    .trim_start_matches('0');
                let nb = std::str::from_utf8(&b[sj..j])
                    .unwrap()
                    .trim_start_matches('0');
                let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                i += 1;
                j += 1;
            }
        }
    }
}

/// The entries in DIR whose name ends in SUFFIX, sorted by name.
fn entries_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(suffix))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The s6 entry made from the systemd entry TEXT: its title, the options
/// without rhgb and quiet and with ARGS, and (if built) the s6 initramfs on
/// the initrd line (a tuned or microcode image beside it stays).
fn s6_entry(text: &str, kver: &str, args: &str, s6_initrd: bool) -> String {
    let mut out = String::new();
    for l in text.lines() {
        let line = if l.starts_with("title ") {
            format!("title {TITLE} {kver}")
        } else if l.starts_with("options ") {
            let o = l.replacen(" rhgb", "", 1).replacen(" quiet", "", 1);
            format!("{o} {args}")
        } else if l.starts_with("initrd ") && s6_initrd {
            l.replacen(
                &format!("initramfs-{kver}.img"),
                &format!("initramfs-{kver}-s6.img"),
                1,
            )
        } else {
            l.to_string()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn add(p: &Paths, kver: &str) {
    let Ok(opts) = fs::read_to_string(&p.opts) else {
        return;
    };
    let args: String = uncommented(&opts).iter().map(|l| format!("{l} ")).collect();
    let src = entries_ending(&p.entries, &format!("{kver}.conf"))
        .into_iter()
        .find(|f| {
            !fs::read_to_string(f)
                .unwrap_or_default()
                .lines()
                .any(|l| l.starts_with(&format!("title {TITLE}")))
        });
    let Some(src) = src else { return };
    let stem = src.to_string_lossy().trim_end_matches(".conf").to_string();
    // "~" sorts before anything, also the end of the name, in both ls -v
    // (grubby's BLS listing) and rpmvercmp (GRUB's blscfg): the systemd
    // entry stays first, so "grubby --set-default KERNEL" on a kernel update
    // keeps picking it. "-uinitconverter" sorted first; drop that name.
    let _ = fs::remove_file(format!("{stem}-uinitconverter.conf"));
    let dest = format!("{stem}~uinitconverter.conf");
    // The s6 initramfs replaces Fedora's on the initrd line. If it can't be
    // built, the entry keeps Fedora's and still boots s6, with systemd in
    // the initramfs.
    let img = p.boot.join(format!("initramfs-{kver}-s6.img"));
    let mut s6_initrd = false;
    if p.only_s6.exists() && p.boot.join(format!("initramfs-{kver}.img")).exists() {
        if run(&p.mkinitramfs, &["-k", kver, "-o", &img.to_string_lossy()]) == Some(0) {
            s6_initrd = true;
        } else {
            eprintln!(
                "90-uinitconverter: building the s6 initramfs for {kver} failed; the s6 entry uses Fedora's"
            );
            let _ = fs::remove_file(&img);
        }
    } else {
        let _ = fs::remove_file(&img);
    }
    let text = fs::read_to_string(&src).unwrap_or_default();
    if fs::write(&dest, s6_entry(&text, kver, &args, s6_initrd)).is_err() {
        eprintln!("90-uinitconverter: can't write {dest}");
        return;
    }
    // On a kernel update 20-grub.install names the new systemd entry in
    // tmp_saved_entry, and 95-set-boot-entry.install makes that the
    // saved_entry after checking the initrd. Name the s6 entry instead.
    if s6_default(p) {
        let id = Path::new(&stem)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if envget(p, "tmp_saved_entry").as_deref() == Some(id.as_str()) {
            envset(p, &format!("tmp_saved_entry={id}~uinitconverter"));
        }
    }
}

fn remove(p: &Paths, kver: &str) {
    let saved = envget(p, "saved_entry");
    for suffix in [
        format!("{kver}~uinitconverter.conf"),
        format!("{kver}-uinitconverter.conf"),
    ] {
        for f in entries_ending(&p.entries, &suffix) {
            let _ = fs::remove_file(f);
        }
    }
    let _ = fs::remove_file(p.boot.join(format!("initramfs-{kver}-s6.img")));
    // The default s6 entry went with its kernel: the newest s6 entry left
    // (GRUB would otherwise fall back to the first entry, a systemd one).
    let Some(saved) = saved.filter(|s| !s.is_empty()) else {
        return;
    };
    if s6_default(p)
        && saved.ends_with("~uinitconverter")
        && !p.entries.join(format!("{saved}.conf")).exists()
    {
        let mut left: Vec<String> = entries_ending(&p.entries, "~uinitconverter.conf")
            .iter()
            .filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        left.sort_by(|a, b| vercmp(a, b));
        if let Some(new) = left.last() {
            envset(p, &format!("saved_entry={}", new.trim_end_matches(".conf")));
        }
    }
}

/// `90-uinitconverter.install COMMAND KVER [IMAGE ...]`: always exits 0,
/// so a kernel installs whatever happens to the s6 entry.
pub fn main(args: &[String]) -> ! {
    let p = Paths::from_env();
    if !p.entries.is_dir() {
        exit(0);
    }
    let kver = args.get(1).map(String::as_str).unwrap_or("");
    match args.first().map(String::as_str) {
        Some("add") if !kver.is_empty() => add(&p, kver),
        Some("remove") if !kver.is_empty() => remove(&p, kver),
        _ => {}
    }
    exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_sort_as_ls_v() {
        let mut v = vec![
            "a-6.10.0~uinitconverter.conf",
            "a-6.9.0~uinitconverter.conf",
            "a-6.10.0.conf",
            "a-6.9.12~uinitconverter.conf",
        ];
        v.sort_by(|a, b| vercmp(a, b));
        assert_eq!(
            v,
            [
                "a-6.9.0~uinitconverter.conf",
                "a-6.9.12~uinitconverter.conf",
                "a-6.10.0~uinitconverter.conf",
                "a-6.10.0.conf",
            ]
        );
        assert_eq!(vercmp("x~", "x"), Ordering::Less);
    }

    #[test]
    fn entry_rewrite() {
        let sys = "title Fedora Linux (6.1) 45\nversion 6.1\nlinux /vmlinuz-6.1\ninitrd /initramfs-6.1.img $tuned_initrd\noptions root=UUID=x ro rhgb quiet\n";
        let e = s6_entry(sys, "6.1", "init=/usr/sbin/init-selinux ", true);
        assert_eq!(
            e,
            "title Fedora (s6 + uinitconverter) 6.1\nversion 6.1\nlinux /vmlinuz-6.1\ninitrd /initramfs-6.1-s6.img $tuned_initrd\noptions root=UUID=x ro init=/usr/sbin/init-selinux \n"
        );
        assert!(s6_entry(sys, "6.1", "", false).contains("initrd /initramfs-6.1.img"));
    }

    #[test]
    fn add_and_remove_in_a_directory() {
        let t =
            std::env::temp_dir().join(format!("uinitconverter-kinstall-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        let e = t.join("entries");
        fs::create_dir_all(&e).unwrap();
        fs::write(e.join("m-6.1.conf"), "title Fedora 6.1\noptions ro quiet\n").unwrap();
        fs::write(t.join("opts"), "# c\ninit=/x\n").unwrap();
        let p = Paths {
            entries: e.clone(),
            opts: t.join("opts"),
            default: t.join("default"),
            editenv: "/nonexistent".into(),
            only_s6: t.join("only"),
            mkinitramfs: "/nonexistent".into(),
            boot: t.clone(),
        };
        add(&p, "6.1");
        let s6 = fs::read_to_string(e.join("m-6.1~uinitconverter.conf")).unwrap();
        assert_eq!(
            s6,
            "title Fedora (s6 + uinitconverter) 6.1\noptions ro  init=/x \n"
        );
        // A second add doesn't take the s6 entry as its source.
        add(&p, "6.1");
        assert_eq!(entries_ending(&e, ".conf").len(), 2);
        remove(&p, "6.1");
        assert_eq!(entries_ending(&e, ".conf").len(), 1);
        let _ = fs::remove_dir_all(&t);
    }
}
