//! The s6 initramfs (initconvert init --boot-only-s6=yes), in Rust (rule 6):
//! the builder (`initconvert mkinitramfs`, also reached as
//! /usr/libexec/uinitconverter/mkinitramfs, a link here) and the two steps
//! inside the image (`initconvert initramfs root|switch`, the run of the
//! rootfs service and s6-svscan's finish). s6-svscan is the image's PID 1
//! (its /init is execline), mdevd loads the drivers; no systemd and no
//! dracut run at boot: dracut-install only copies binaries with their
//! libraries and kernel modules with their dependencies at build time.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{symlink, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};
use std::time::Duration;

const DI: &str = "/usr/lib/dracut/dracut-install";
const SHARE_TREE: &str = "/usr/share/uinitconverter/initramfs";

/// The build's work directory, removed however the build ends.
static WORK: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn die(msg: &str) -> ! {
    eprintln!("mkinitramfs: {msg}");
    if let Some(w) = WORK.get() {
        let _ = fs::remove_dir_all(w);
    }
    exit(1)
}

const MK_USAGE: &str = "\
Build the s6 initramfs (initconvert init --boot-only-s6=yes): s6-svscan is its
PID 1, mdevd loads the drivers, and initconvert opens LUKS, if any, and mounts
the root (the tree in /usr/share/uinitconverter/initramfs).

  mkinitramfs [options]
    -k KVER           kernel version (default: uname -r)
    -o FILE           output (default: /boot/initramfs-KVER-s6.img)
    --source DIR      the initramfs tree (default: /usr/share/uinitconverter/initramfs)
    --add-module M    another kernel module (or =DIR of them); repeatable
    --add FILE        another file or program (with its libraries); repeatable
    --no-microcode    no early CPU microcode

Extra modules are also read from /etc/uinitconverter/initramfs-modules (one name
or =DIR per line). cryptsetup goes in when it's installed. Without it the image
has no LUKS support, and building one is refused when /etc/crypttab has an
entry or the kernel command line has rd.luks.uuid= or rd.luks.name=.
Runs unprivileged when the output is writable.
";

struct Builder {
    root: PathBuf,
    path: String,
}

impl Builder {
    fn di(&self, args: &[&str]) -> bool {
        let mut a = vec!["-D", self.root.to_str().unwrap()];
        a.extend_from_slice(args);
        Command::new(DI)
            .args(&a)
            .env("PATH", &self.path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// One program or file with its libraries. A symlink to a multicall
    /// binary (execline's programs, kmod's modprobe) is copied as the link
    /// plus its target: dracut-install mixes such links up when given many.
    fn inst(&self, p: &str) {
        if let Ok(t) = fs::read_link(p) {
            let target = if t.is_absolute() {
                t.to_string_lossy().into_owned()
            } else {
                Path::new(p)
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .join(&t)
                    .to_string_lossy()
                    .into_owned()
            };
            self.inst(&target);
            let dest = self.root.join(p.trim_start_matches('/'));
            if let Some(d) = dest.parent() {
                let _ = fs::create_dir_all(d);
            }
            if fs::symlink_metadata(&dest).is_err() {
                let _ = symlink(&t, &dest);
            }
        } else if !self.di(&["-l", p]) {
            die(&format!("dracut-install couldn't copy {p}"));
        }
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(&self.path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    }

    /// The program NAME as found in PATH (its directory resolved: /sbin/x is
    /// installed as /usr/bin/x on Fedora). One found outside /usr (a test's
    /// PATH) goes to /usr/bin.
    fn prog(&self, name: &str) {
        let Some(p) = self.which(name) else {
            die(&format!("{name} not found"))
        };
        let dir = fs::canonicalize(p.parent().unwrap()).unwrap_or_default();
        if dir.starts_with("/usr/") {
            self.inst(&dir.join(name).to_string_lossy());
        } else {
            let real = fs::canonicalize(&p).unwrap_or(p);
            if !self.di(&["-l", &real.to_string_lossy(), &format!("/usr/bin/{name}")]) {
                die(&format!("dracut-install couldn't copy {name}"));
            }
        }
    }
}

/// Every path under DIR (DIR itself as "."), relative, sorted: what `find .
/// -print0 | sort -z` gives cpio, directories before what they hold.
fn tree_list(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, rel: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(base.join(rel)) else {
            return;
        };
        for e in rd.flatten() {
            let r = rel.join(e.file_name());
            out.push(format!("./{}", r.display()));
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(base, &r, out);
            }
        }
    }
    let mut out = vec![".".to_string()];
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

/// A newc cpio archive of DIR (owned by root whoever builds it).
fn cpio(dir: &Path) -> Vec<u8> {
    let list: Vec<u8> = tree_list(dir)
        .into_iter()
        .flat_map(|p| {
            let mut b = p.into_bytes();
            b.push(0);
            b
        })
        .collect();
    let mut c = Command::new("cpio")
        .args(["--null", "-o", "-H", "newc", "-R", "0:0", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| die(&format!("cpio: {e}")));
    let mut stdin = c.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&list);
    });
    let mut out = Vec::new();
    let _ = c.stdout.take().unwrap().read_to_end(&mut out);
    let _ = writer.join();
    if !c.wait().map(|s| s.success()).unwrap_or(false) {
        die("cpio failed");
    }
    out
}

fn zstd(data: Vec<u8>) -> Vec<u8> {
    let mut c = Command::new("zstd")
        .args(["-q", "-15", "-T0", "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| die(&format!("zstd: {e}")));
    let mut stdin = c.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let mut out = Vec::new();
    let _ = c.stdout.take().unwrap().read_to_end(&mut out);
    let _ = writer.join();
    if !c.wait().map(|s| s.success()).unwrap_or(false) {
        die("zstd failed");
    }
    out
}

/// The early microcode file for this CPU (/proc/cpuinfo's first entry).
fn microcode_file(cpuinfo: &str) -> Option<(String, String)> {
    let field = |k: &str| {
        cpuinfo.lines().find_map(|l| {
            let (a, b) = l.split_once(':')?;
            (a.trim() == k).then(|| b.trim().to_string())
        })
    };
    let vendor = field("vendor_id")?;
    let num = |k: &str| field(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (fam, model, step) = (num("cpu family"), num("model"), num("stepping"));
    let path = match vendor.as_str() {
        "AuthenticAMD" if fam >= 21 => {
            format!("/usr/lib/firmware/amd-ucode/microcode_amd_fam{fam:02x}h.bin")
        }
        "AuthenticAMD" => "/usr/lib/firmware/amd-ucode/microcode_amd.bin".to_string(),
        "GenuineIntel" => format!("/usr/lib/firmware/intel-ucode/{fam:02x}-{model:02x}-{step:02x}"),
        _ => return None,
    };
    Some((vendor, path))
}

/// Whether CRYPTTAB has an entry or CMDLINE names LUKS volumes.
fn wants_luks(crypttab: &str, cmdline: &str) -> bool {
    crypttab
        .lines()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        || cmdline
            .split_whitespace()
            .any(|w| w.starts_with("rd.luks.uuid=") || w.starts_with("rd.luks.name="))
}

pub fn mkinitramfs(args: &[String]) -> ! {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    // Programs come from the system, not from the caller's PATH (tests set
    // UINITCONVERTER_INITRAMFS_PATH to use tools that are not installed).
    let path = env("UINITCONVERTER_INITRAMFS_PATH", "/usr/bin:/usr/sbin");
    let modules_file = env(
        "UINITCONVERTER_INITRAMFS_MODULES",
        "/etc/uinitconverter/initramfs-modules",
    );
    let crypttab = env("UINITCONVERTER_INITRAMFS_CRYPTTAB", "/etc/crypttab");
    let cmdline_file = env("UINITCONVERTER_INITRAMFS_CMDLINE", "/proc/cmdline");
    let mut kver = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let (mut out, mut src) = (None, None);
    let mut microcode = true;
    let (mut extra_modules, mut extra_files) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .unwrap_or_else(|| die(&format!("{a} needs a value")))
        };
        match a.as_str() {
            "-k" => kver = val(),
            "-o" => out = Some(val()),
            "--source" => src = Some(val()),
            "--add-module" => extra_modules.push(val()),
            "--add" => extra_files.push(val()),
            "--no-microcode" => microcode = false,
            "-h" | "--help" => {
                print!("{MK_USAGE}");
                exit(0)
            }
            o => die(&format!("unknown option: {o}")),
        }
    }
    let out = out.unwrap_or_else(|| format!("/boot/initramfs-{kver}-s6.img"));
    let src = PathBuf::from(src.unwrap_or_else(|| env("UINITCONVERTER_INITRAMFS_SOURCE", SHARE_TREE)));
    let kdir = PathBuf::from(format!("/lib/modules/{kver}"));
    if !src.is_dir() {
        die(&format!("{}: no initramfs tree", src.display()));
    }
    if !kdir.is_dir() {
        die(&format!("{}: no modules for kernel {kver}", kdir.display()));
    }
    if !Path::new(DI).exists() {
        die(&format!("{DI} missing (dnf install dracut)"));
    }
    let work =
        std::env::temp_dir().join(format!("uinitconverter-initramfs.{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    let _ = WORK.set(work.clone());
    let b = Builder {
        root: work.join("root"),
        path: path.clone(),
    };
    for p in ["cpio", "zstd", "depmod", "mdevd", "s6-svscan", "execlineb"] {
        if b.which(p).is_none() {
            die(&format!("{p} not found"));
        }
    }
    // An image that couldn't open the root must fail here, not at the next boot.
    let have_crypt = b.which("cryptsetup").is_some();
    if !have_crypt {
        if wants_luks(
            &fs::read_to_string(&crypttab).unwrap_or_default(),
            &fs::read_to_string(&cmdline_file).unwrap_or_default(),
        ) {
            die(&format!(
                "cryptsetup not found, and {crypttab} or the kernel command line names encrypted volumes"
            ));
        }
        eprintln!("mkinitramfs: cryptsetup not found: this image can't open LUKS volumes");
    }
    build(
        &b,
        &BuildArgs {
            src: &src,
            kver: &kver,
            kdir: &kdir,
            out: &out,
            microcode,
            extra_modules: &extra_modules,
            extra_files: &extra_files,
            modules_file: &modules_file,
            crypttab: &crypttab,
            have_crypt,
        },
    );
    let _ = fs::remove_dir_all(&work);
    exit(0)
}

struct BuildArgs<'a> {
    src: &'a Path,
    kver: &'a str,
    kdir: &'a Path,
    out: &'a str,
    microcode: bool,
    extra_modules: &'a [String],
    extra_files: &'a [String],
    modules_file: &'a str,
    crypttab: &'a str,
    have_crypt: bool,
}

fn build(b: &Builder, a: &BuildArgs) {
    let r = &b.root;
    // The host's /usr layout (Fedora: /bin, /sbin, /lib, /lib64 and
    // /usr/sbin are symlinks), so paths in scripts and libraries resolve the
    // same way.
    for d in ["usr/bin", "usr/lib", "usr/lib64"] {
        let _ = fs::create_dir_all(r.join(d));
    }
    for d in ["usr/sbin", "bin", "sbin", "lib", "lib64"] {
        match fs::read_link(Path::new("/").join(d)) {
            Ok(t) => {
                let _ = symlink(t, r.join(d));
            }
            Err(_) => {
                let _ = fs::create_dir_all(r.join(d));
            }
        }
    }
    for d in [
        "dev",
        "proc",
        "sys",
        "run",
        "sysroot",
        "tmp",
        "etc",
        "usr/lib/firmware",
    ] {
        let _ = fs::create_dir_all(r.join(d));
    }
    let ok = Command::new("cp")
        .arg("-a")
        .arg(a.src.join("."))
        .arg(r)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        die(&format!("copying {} failed", a.src.display()));
    }
    // What /init, the services, mdev.conf and the rescue shell run.
    for p in [
        "execlineb",
        "export",
        "foreground",
        "redirfd",
        "fdmove",
        "importas",
        "s6-svscan",
        "s6-supervise",
        "s6-svscanctl",
        "s6-svc",
        "s6-svwait",
        "mdevd",
        "mdevd-coldplug",
        "bash",
        "mount",
        "umount",
        "mountpoint",
        "switch_root",
        "setsid",
        "blkid",
        "modprobe",
        "insmod",
        "lsmod",
        "depmod",
        "printf",
        "cat",
        "sleep",
        "mkdir",
        "ls",
        "ln",
        "rm",
        "dmesg",
    ] {
        b.prog(p);
    }
    if b.which("btrfs").is_some() {
        b.prog("btrfs");
    }
    if a.have_crypt {
        b.prog("cryptsetup");
    }
    // The rootfs service and s6-svscan's finish: this program.
    let me = std::env::current_exe().unwrap_or_else(|e| die(&format!("this program: {e}")));
    if !b.di(&["-l", &me.to_string_lossy(), "/usr/bin/initconvert"]) {
        die("dracut-install couldn't copy initconvert");
    }
    // s6-svwait (rootfs waits for mdevd) starts s6's ftrigrd helper.
    let ftrigrd = "/usr/libexec/s6/s6-ftrigrd";
    if !Path::new(ftrigrd).exists() {
        die(&format!("{ftrigrd} missing (s6's helper for s6-svwait)"));
    }
    b.inst(ftrigrd);
    let sh = r.join("usr/bin/sh");
    let _ = fs::remove_file(&sh);
    let _ = symlink("bash", &sh);
    // glibc loads libgcc_s for thread cancellation (cryptsetup's argon2 threads).
    if Path::new("/usr/lib64/libgcc_s.so.1").exists() {
        b.inst("/usr/lib64/libgcc_s.so.1");
    }
    for f in a.extra_files {
        b.inst(f);
    }
    if Path::new(a.crypttab).exists() {
        let _ = fs::copy(a.crypttab, r.join("etc/crypttab"));
    }
    // Kernel modules: what can sit between the CPU and the root filesystem
    // or the keyboard. mdevd loads only those the hardware asks for.
    let mut modules: Vec<String> = [
        "=drivers/nvme",
        "=drivers/ata",
        "=drivers/scsi",
        "=drivers/md",
        "=drivers/block",
        "=drivers/mmc",
        "=drivers/virtio",
        "=drivers/pci/controller",
        "=drivers/usb/host",
        "=drivers/usb/storage",
        "=drivers/hid",
        "=drivers/input/keyboard",
        "=drivers/input/serio",
        "=crypto",
        "=arch/x86/crypto",
        "=lib/crypto",
        "dm_mod",
        "dm_crypt",
        "btrfs",
        "ext4",
        "xfs",
        "vfat",
        "nls_cp437",
        "nls_iso8859-1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for l in fs::read_to_string(a.modules_file)
        .unwrap_or_default()
        .lines()
    {
        if let Some(m) = l.split_whitespace().next().filter(|m| !m.starts_with('#')) {
            modules.push(m.to_string());
        }
    }
    modules.extend(a.extra_modules.iter().cloned());
    let kv = a.kver;
    let fw = format!(
        "/usr/lib/firmware/updates/{kv}:/usr/lib/firmware/updates:/usr/lib/firmware/{kv}:/usr/lib/firmware"
    );
    let mut di = vec![
        "--kerneldir".to_string(),
        a.kdir.to_string_lossy().into_owned(),
        "--firmwaredirs".into(),
        fw,
        "-o".into(),
        "-m".into(),
    ];
    di.extend(modules);
    let dia: Vec<&str> = di.iter().map(String::as_str).collect();
    b.di(&dia);
    let mdir = r.join(format!("usr/lib/modules/{kv}"));
    let _ = fs::create_dir_all(&mdir);
    for f in [
        "modules.builtin",
        "modules.builtin.modinfo",
        "modules.order",
    ] {
        let s = a.kdir.join(f);
        if s.exists() {
            let _ = fs::copy(&s, mdir.join(f));
        }
    }
    Command::new("depmod")
        .args(["-b", &r.to_string_lossy(), kv])
        .env("PATH", &b.path)
        .status()
        .ok();
    // Early microcode: an uncompressed cpio in front, as the kernel wants it.
    let mut image = Vec::new();
    if a.microcode {
        if let Some((vendor, uc)) =
            microcode_file(&fs::read_to_string("/proc/cpuinfo").unwrap_or_default())
        {
            if Path::new(&uc).exists() {
                let e = r.parent().unwrap().join("early");
                let d = e.join("kernel/x86/microcode");
                let _ = fs::create_dir_all(&d);
                let _ = fs::copy(&uc, d.join(format!("{vendor}.bin")));
                image.extend(cpio(&e));
            }
        }
    }
    image.extend(zstd(cpio(r)));
    // Owned by root in the archive whoever builds it; 0600.
    let tmp = format!("{}.tmp.{}", a.out, std::process::id());
    let w = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(&image));
    if w.is_err() {
        let _ = fs::remove_file(&tmp);
        die(&format!("writing {} failed", a.out));
    }
    if fs::rename(&tmp, a.out).is_err() {
        let _ = fs::remove_file(&tmp);
        die(&format!("writing {} failed", a.out));
    }
    println!(
        "mkinitramfs: {} ({}M, kernel {kv})",
        a.out,
        image.len().div_ceil(1 << 20)
    );
}

// ------------------------------------------------------------ in the image

const SYSROOT: &str = "/sysroot";

fn say(msg: &str) {
    println!("initramfs: {msg}");
}

fn shell() {
    let _ = Command::new("setsid").args(["-c", "bash", "-i"]).status();
}

/// A rescue shell on the console; when it exits, s6-supervise runs the
/// rootfs service again.
fn rescue(msg: &str) -> ! {
    say(msg);
    say(&format!(
        "rescue shell: fix it and exit to try again (the root goes to {SYSROOT})"
    ));
    shell();
    exit(1)
}

/// The kernel options the root step reads.
#[derive(Debug, PartialEq)]
struct RootArgs {
    root: Option<String>,
    fstype: String,
    flags: String,
    mode: String,
    init: String,
    timeout: u64,
    brk: bool,
    luks: bool,
    /// LUKS UUID -> mapping name.
    luks_name: BTreeMap<String, String>,
}

/// root= (UUID=, LABEL=, PARTUUID=, PARTLABEL=, /dev/...), rootfstype=,
/// rootflags=, ro/rw, init=, rd.luks.uuid= (luks-UUID or UUID),
/// rd.luks.name=UUID=NAME, rd.luks=0, rd.break (shell before switching),
/// uinitconverter.initrd.timeout=SECONDS (wait for disks, default 30).
fn root_args(cmdline: &str) -> RootArgs {
    let mut a = RootArgs {
        root: None,
        fstype: "auto".into(),
        flags: String::new(),
        mode: "ro".into(),
        init: "/sbin/init".into(),
        timeout: 30,
        brk: false,
        luks: true,
        luks_name: BTreeMap::new(),
    };
    for w in cmdline.split_whitespace() {
        if let Some(v) = w.strip_prefix("root=") {
            a.root = Some(v.into());
        } else if let Some(v) = w.strip_prefix("rootfstype=") {
            a.fstype = v.into();
        } else if let Some(v) = w.strip_prefix("rootflags=") {
            if !a.flags.is_empty() {
                a.flags.push(',');
            }
            a.flags.push_str(v);
        } else if w == "ro" || w == "rw" {
            a.mode = w.into();
        } else if let Some(v) = w.strip_prefix("init=") {
            a.init = v.into();
        } else if w == "rd.luks=0" {
            a.luks = false;
        } else if let Some(v) = w.strip_prefix("rd.luks.uuid=") {
            let u = v.strip_prefix("luks-").unwrap_or(v);
            a.luks_name.insert(u.into(), format!("luks-{u}"));
        } else if let Some((u, n)) = w
            .strip_prefix("rd.luks.name=")
            .and_then(|v| v.split_once('='))
        {
            a.luks_name.insert(u.into(), n.into());
        } else if w == "rd.break" {
            a.brk = true;
        } else if let Some(v) = w.strip_prefix("uinitconverter.initrd.timeout=") {
            a.timeout = v.parse().unwrap_or(30);
        }
    }
    a
}

/// No udev here makes no /dev/disk links: those are looked up as blkid tags.
fn blkid_spec(spec: &str) -> String {
    for (pre, tag) in [
        ("/dev/disk/by-uuid/", "UUID"),
        ("/dev/disk/by-label/", "LABEL"),
        ("/dev/disk/by-partuuid/", "PARTUUID"),
        ("/dev/disk/by-partlabel/", "PARTLABEL"),
    ] {
        if let Some(v) = spec.strip_prefix(pre) {
            return format!("{tag}={v}");
        }
    }
    spec.to_string()
}

fn blkid_device(tag: &str) -> Option<String> {
    let o = Command::new("blkid")
        .args(["-l", "-o", "device", "-t", tag])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn is_block(p: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(p)
        .map(|m| m.file_type().is_block_device())
        .unwrap_or(false)
}

/// The device for a root=-style SPEC, waiting up to TIMEOUT seconds.
fn find_dev(spec: &str, timeout: u64) -> Option<String> {
    let spec = blkid_spec(spec);
    let mut n = 0;
    loop {
        let dev = if ["UUID=", "LABEL=", "PARTUUID=", "PARTLABEL="]
            .iter()
            .any(|p| spec.starts_with(p))
        {
            blkid_device(&spec)
        } else if spec.starts_with("/dev/") {
            is_block(&spec).then(|| spec.clone())
        } else {
            blkid_device(&format!("UUID={spec}"))
        };
        if dev.is_some() {
            return dev;
        }
        if n >= timeout * 4 {
            return None;
        }
        if n == 8 {
            eprintln!("initramfs: waiting for {spec} ...");
        }
        n += 1;
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Options from /etc/crypttab (copied from the host): only discard matters.
fn crypt_opts(crypttab: &str, uuid: &str, name: &str) -> Vec<String> {
    for l in crypttab.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        let Some(n) = f.first() else { continue };
        if n.starts_with('#') {
            continue;
        }
        let dev = f.get(1).copied().unwrap_or("");
        if dev != format!("UUID={uuid}") && *n != name {
            continue;
        }
        if f.get(3)
            .is_some_and(|o| o.split(',').any(|x| x == "discard"))
        {
            return vec!["--allow-discards".into()];
        }
    }
    Vec::new()
}

/// Whether NAME is a program in PATH (the image's /usr/bin:/usr/sbin).
fn in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

fn ok(prog: &str, args: &[&str]) -> bool {
    Command::new(prog)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The rootfs service: open the LUKS volumes named on the kernel command
/// line, if any, mount the root on /sysroot, then stop s6-svscan, whose
/// finish (`initramfs switch`) switches to it. On any failure: a rescue
/// shell on the console.
fn root() -> ! {
    std::env::set_var("DM_DISABLE_UDEV", "1");
    // SAFETY: dup2 of this process's own descriptors (stderr to stdout).
    unsafe { libc::dup2(1, 2) };
    let a = root_args(&fs::read_to_string("/proc/cmdline").unwrap_or_default());
    let Some(rootspec) = a.root.clone() else {
        rescue("no root= on the kernel command line")
    };
    // mdevd loads the drivers; the coldplug replays what came before it.
    if !ok("s6-svwait", &["-U", "-t", "10000", "/service/mdevd"]) {
        say("mdevd is not ready; trying anyway");
    }
    ok("mdevd-coldplug", &[]);
    if a.luks && !a.luks_name.is_empty() {
        if !in_path("cryptsetup") {
            rescue("rd.luks.* is on the kernel command line, but this initramfs has no cryptsetup");
        }
        ok("modprobe", &["-q", "dm_crypt"]);
        let _ = fs::create_dir_all("/run/cryptsetup");
        let crypttab = fs::read_to_string("/etc/crypttab").unwrap_or_default();
        for (u, name) in &a.luks_name {
            if is_block(&format!("/dev/mapper/{name}")) {
                continue;
            }
            let Some(dev) = find_dev(&format!("UUID={u}"), a.timeout) else {
                rescue(&format!("LUKS device UUID={u} not found"))
            };
            let mut args = vec![
                "open".to_string(),
                "--type".into(),
                "luks".into(),
                "--tries".into(),
                "3".into(),
            ];
            args.extend(crypt_opts(&crypttab, u, name));
            args.push(dev.clone());
            args.push(name.clone());
            let av: Vec<&str> = args.iter().map(String::as_str).collect();
            if !ok("cryptsetup", &av) {
                rescue(&format!("could not open {dev}"));
            }
        }
    }
    // Fedora's 10-dm.rules ignore a coldplug "add" event for a dm device
    // unless the udev database says it was activated
    // (DM_UDEV_PRIMARY_SOURCE_FLAG): the real system's coldplug would give
    // the LUKS mapping no /dev/disk/by-* links. A systemd initramfs leaves
    // its udev database in /run; this leaves the record udev would have
    // made when the device was activated.
    let _ = fs::create_dir_all("/run/udev/data");
    if let Ok(rd) = fs::read_dir("/sys/block") {
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().starts_with("dm-") {
                continue;
            }
            if let Ok(d) = fs::read_to_string(e.path().join("dev")) {
                let _ = fs::write(
                    format!("/run/udev/data/b{}", d.trim()),
                    "E:DM_UDEV_PRIMARY_SOURCE_FLAG=1\nE:DM_UDEV_RULES_VSN=2\n",
                );
            }
        }
    }
    let Some(dev) = find_dev(&rootspec, a.timeout) else {
        rescue(&format!("root device {rootspec} not found"))
    };
    // A btrfs root may span devices: register them all before mounting.
    if in_path("btrfs") {
        let ty = Command::new("blkid")
            .args(["-o", "value", "-s", "TYPE", &dev])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if a.fstype == "btrfs" || ty == "btrfs" {
            let _ = Command::new("btrfs")
                .args(["device", "scan"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _ = fs::create_dir_all(SYSROOT);
    if !ok("mountpoint", &["-q", SYSROOT]) {
        let opts = if a.flags.is_empty() {
            a.mode.clone()
        } else {
            format!("{},{}", a.mode, a.flags)
        };
        if !ok("mount", &["-t", &a.fstype, "-o", &opts, &dev, SYSROOT]) {
            rescue(&format!("could not mount {dev} on {SYSROOT}"));
        }
    }
    let target = format!("{SYSROOT}{}", a.init);
    if fs::symlink_metadata(&target).is_err() {
        rescue(&format!("no {} on the root", a.init));
    }
    if a.brk {
        say("rd.break: shell before switching to the root");
        shell();
    }
    // finish (s6-svscan's last act, as PID 1) execs the real init.
    let _ = fs::write("/init-target", format!("{}\n", a.init));
    ok("s6-svc", &["-O", "/service/rootfs"]);
    let e = Command::new("s6-svscanctl").args(["-t", "/service"]).exec();
    rescue(&format!("s6-svscanctl: {e}"))
}

/// The kernel's arguments for init, as /init kept them (one per line).
fn init_args(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// s6-svscan's finish, as PID 1 when it stops: normally after the root
/// step mounted the root, then switch to it. Without a root (s6-svscan was
/// stopped some other way) there is nothing to switch to: a shell, as PID 1
/// must not exit.
fn switch() -> ! {
    if let Ok(f) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/console")
    {
        use std::os::unix::io::AsRawFd;
        let fd = f.as_raw_fd();
        // SAFETY: dup2 onto the standard descriptors.
        unsafe {
            libc::dup2(fd, 0);
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }
    let init = fs::read_to_string("/init-target")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if init.is_empty() || !ok("mountpoint", &["-q", SYSROOT]) {
        println!("initramfs: s6-svscan stopped without a root to switch to");
        loop {
            let _ = Command::new("bash").arg("-i").status();
        }
    }
    let args = init_args(&fs::read_to_string("/init-args").unwrap_or_default());
    for m in ["dev", "proc", "sys", "run"] {
        ok(
            "mount",
            &["--move", &format!("/{m}"), &format!("{SYSROOT}/{m}")],
        );
    }
    let e = Command::new("switch_root")
        .arg(SYSROOT)
        .arg(&init)
        .args(&args)
        .exec();
    println!("initramfs: switch_root: {e}");
    loop {
        let _ = Command::new("bash").arg("-i").status();
    }
}

pub fn main(args: &[String]) -> ! {
    match args.first().map(String::as_str).unwrap_or("") {
        "root" => root(),
        "switch" => switch(),
        _ => {
            eprintln!("usage: initconvert initramfs root|switch");
            exit(100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_options() {
        let a = root_args(
            "BOOT_IMAGE=x root=UUID=r rootflags=subvol=root rootflags=compress=zstd rw init=/usr/sbin/init-selinux \
             rd.luks.uuid=luks-aaa rd.luks.name=bbb=data rd.break uinitconverter.initrd.timeout=5",
        );
        assert_eq!(a.root.as_deref(), Some("UUID=r"));
        assert_eq!(a.flags, "subvol=root,compress=zstd");
        assert_eq!(a.mode, "rw");
        assert_eq!(a.init, "/usr/sbin/init-selinux");
        assert_eq!(a.timeout, 5);
        assert!(a.brk && a.luks);
        assert_eq!(a.luks_name.get("aaa").map(String::as_str), Some("luks-aaa"));
        assert_eq!(a.luks_name.get("bbb").map(String::as_str), Some("data"));
        let d = root_args("ro rd.luks=0");
        assert_eq!(
            (d.root, d.fstype.as_str(), d.init.as_str(), d.luks),
            (None, "auto", "/sbin/init", false)
        );
    }

    #[test]
    fn specs_and_crypttab() {
        assert_eq!(blkid_spec("/dev/disk/by-uuid/x"), "UUID=x");
        assert_eq!(blkid_spec("/dev/disk/by-partlabel/root"), "PARTLABEL=root");
        assert_eq!(blkid_spec("/dev/vda2"), "/dev/vda2");
        let ct = "# c\nluks-aaa UUID=aaa none discard,tries=1\nother UUID=ccc none\n";
        assert_eq!(crypt_opts(ct, "aaa", "x"), ["--allow-discards"]);
        assert!(crypt_opts(ct, "ccc", "other").is_empty());
        assert!(wants_luks(ct, ""));
        assert!(wants_luks("", "ro rd.luks.uuid=x"));
        assert!(!wants_luks("# only\n\n", "ro"));
    }

    #[test]
    fn microcode_names() {
        let amd = "vendor_id\t: AuthenticAMD\ncpu family\t: 25\nmodel\t\t: 33\nstepping\t: 0\n";
        assert_eq!(
            microcode_file(amd).unwrap().1,
            "/usr/lib/firmware/amd-ucode/microcode_amd_fam19h.bin"
        );
        let intel = "vendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t\t: 151\nstepping\t: 2\n";
        assert_eq!(
            microcode_file(intel).unwrap().1,
            "/usr/lib/firmware/intel-ucode/06-97-02"
        );
    }

    #[test]
    fn listing_order() {
        let t = std::env::temp_dir().join(format!("uinitconverter-cpio-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        fs::create_dir_all(t.join("b/c")).unwrap();
        fs::write(t.join("a"), "").unwrap();
        fs::write(t.join("b/c/d"), "").unwrap();
        assert_eq!(tree_list(&t), [".", "./a", "./b", "./b/c", "./b/c/d"]);
        let _ = fs::remove_dir_all(&t);
        assert_eq!(init_args("a\n\nb c\n"), ["a", "b c"]);
    }
}
