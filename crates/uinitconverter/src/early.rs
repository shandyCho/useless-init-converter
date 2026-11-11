//! Early-boot steps for the s6 services in /etc/uinitconverter/s6 (their
//! `up`/`down` run `/usr/libexec/uinitconverter/early STEP`, a link to
//! initconvert). They replace the systemd units that only make sense with
//! systemd as PID 1 (systemd-remount-fs, systemd-modules-load, the API
//! filesystem mounts, ...) and the systemd programs other packages can do
//! (rule 2, work_history part 219): systemd-vconsole-setup (kbd),
//! systemd-machine-id-setup (dbus-uuidgen), systemd-user-sessions and
//! systemd-backlight. Ported from the dist/early shell script (rule 6).
//!
//! Every step exits 0: s6-rc treats a failed oneshot as a failed dependency
//! of everything after it, but these steps only degrade the system when they
//! fail (a sysctl key, one tmpfiles line). Failures are logged instead.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

const USAGE: &str = "usage: early mount-api|mount-fs|umount-fs|udev-trigger|modules-load|hostname|tmpfiles|sysusers|sysctl|vconsole|rfkill|rfkill-save|backlight|backlight-save|user-sessions|user-sessions-stop|resolv|zram|zram-stop";

pub(crate) fn warn(msg: &str) {
    eprintln!("uinitconverter early: {msg}");
}

/// Run a program; its exit code, or None if it couldn't start or was killed.
pub(crate) fn run(prog: &str, args: &[&str]) -> Option<i32> {
    Command::new(prog)
        .args(args)
        .status()
        .ok()
        .and_then(|s| s.code())
}

/// Run a program quietly (no stdout/stderr): true if it exited 0.
pub(crate) fn quiet(prog: &str, args: &[&str]) -> bool {
    Command::new(prog)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A program's stdout, if it exited 0.
pub(crate) fn output(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

pub(crate) fn has(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

pub(crate) fn cmdline() -> String {
    fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

fn restore_state_off(cmdline: &str) -> bool {
    cmdline
        .split_whitespace()
        .any(|w| w == "systemd.restore_state=0")
}

pub(crate) fn restorecon(args: &[&str]) {
    if has("restorecon") {
        quiet("restorecon", args);
    }
}

/// mount -t FSTYPE -o OPTIONS FSTYPE TARGET, unless TARGET is a mount point.
fn mnt(fstype: &str, target: &str, opts: &str, silent: bool) {
    if quiet("mountpoint", &["-q", target]) {
        return;
    }
    let _ = fs::create_dir_all(target);
    let ok = if silent {
        quiet("mount", &["-t", fstype, "-o", opts, fstype, target])
    } else {
        run("mount", &["-t", fstype, "-o", opts, fstype, target]) == Some(0)
    };
    if !ok && !silent {
        warn(&format!("mount {fstype} on {target} failed"));
    }
}

fn mknod(path: &str, mode: libc::mode_t) -> bool {
    let c = std::ffi::CString::new(path).unwrap();
    // SAFETY: a valid C string; mknod doesn't keep the pointer.
    unsafe { libc::mknod(c.as_ptr(), mode, 0) == 0 }
}

fn mkdir_mode(path: &str, mode: u32) {
    let _ = fs::DirBuilder::new().mode(mode).create(path);
    // The umask may have taken bits off.
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

/// systemd-tmpfiles' SuccessExitStatus= (DATAERR, CANTCREAT) counts as fine.
fn tmpfiles(args: &[&str], what: &str) {
    match run("/usr/bin/systemd-tmpfiles", args) {
        Some(0 | 65 | 73) => {}
        r => warn(&format!("{what} exited {}", code(r))),
    }
}

fn code(r: Option<i32>) -> String {
    r.map(|c| c.to_string())
        .unwrap_or_else(|| "abnormally".into())
}

fn mount_api() {
    // Usually already mounted by the initramfs; harmless otherwise.
    mnt("proc", "/proc", "nosuid,nodev,noexec", false);
    mnt("sysfs", "/sys", "nosuid,nodev,noexec", false);
    mnt("devtmpfs", "/dev", "nosuid,mode=0755", false);
    mnt("tmpfs", "/run", "nosuid,nodev,mode=0755", false);
    mnt(
        "cgroup2",
        "/sys/fs/cgroup",
        "nosuid,nodev,noexec,nsdelegate",
        false,
    );
    mnt(
        "devpts",
        "/dev/pts",
        "nosuid,noexec,gid=5,mode=0620,ptmxmode=000",
        false,
    );
    mnt("tmpfs", "/dev/shm", "nosuid,nodev,mode=1777", false);
    mnt(
        "securityfs",
        "/sys/kernel/security",
        "nosuid,nodev,noexec",
        true,
    );
    mnt("bpf", "/sys/fs/bpf", "nosuid,nodev,noexec,mode=0700", true);
    mnt("pstore", "/sys/fs/pstore", "nosuid,nodev,noexec", true);
    // PID 1's too: the kernel default is private, and units, flatpak and
    // containers make / a slave in their own namespaces, so a slave of a
    // private tree never sees later mounts (a USB stick under /run/media).
    // Filesystems mounted later (mount-fs) are shared as their parent is.
    if run("mount", &["--make-rshared", "/"]) != Some(0) {
        warn("mount --make-rshared / failed");
    }
    // systemd (PID 1) creates userdb for the services that serve user records
    // there (gdm's org.gnome.DisplayManager); it needs its own label, not
    // /run/systemd's.
    let _ = fs::create_dir_all("/run/systemd/userdb");
    // Also PID 1's: systemd-executor bind-mounts a unit's new root on
    // mount-rootfs and can't create it itself in a user manager's user
    // namespace, so every PrivateTmp/ProtectSystem/... user unit fails with
    // 226/NAMESPACE without it (oo7-daemon). inaccessible/chr and blk are
    // its fallback for nodes it can't mknod there.
    if !Path::new("/run/systemd/mount-rootfs").is_dir() {
        mkdir_mode("/run/systemd/mount-rootfs", 0o555);
    }
    let ina = "/run/systemd/inaccessible";
    if !Path::new(ina).is_dir() {
        mkdir_mode(ina, 0o755);
        let reg = format!("{ina}/reg");
        if fs::write(&reg, "").is_ok() {
            let _ = fs::set_permissions(&reg, fs::Permissions::from_mode(0));
        }
        mkdir_mode(&format!("{ina}/dir"), 0);
        mknod(&format!("{ina}/fifo"), libc::S_IFIFO);
        mknod(&format!("{ina}/chr"), libc::S_IFCHR);
        mknod(&format!("{ina}/blk"), libc::S_IFBLK);
    }
    // Also PID 1's (systemd's mount-setup): devtmpfs has no /dev/fd and
    // stdio links. A systemd initramfs leaves them in the /dev it hands
    // over; the s6 one (--boot-only-s6) doesn't, and without /dev/fd every
    // bash process substitution fails ("/dev/fd/63: No such file").
    for (l, t) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        let p = format!("/dev/{l}");
        if fs::symlink_metadata(&p).is_err() && symlink(t, &p).is_err() {
            warn(&format!("can't create {p}"));
        }
    }
    if Path::new("/proc/kcore").exists() && fs::symlink_metadata("/dev/core").is_err() {
        let _ = symlink("/proc/kcore", "/dev/core");
    }
    // kmod-static-nodes and systemd-tmpfiles-setup-dev-early, which EXCLUDE
    // drops: the nodes of modules that load when their node is first opened
    // (/dev/net/tun for podman and VPNs, /dev/uinput, /dev/vhost-net). The
    // initramfs made only those of its own modules; without these, opening
    // one fails with ENOENT and the module never loads.
    let kver = kernel_release();
    let devname = format!("/lib/modules/{kver}/modules.devname");
    if fs::metadata(&devname).map(|m| m.len() > 0).unwrap_or(false)
        && Path::new("/usr/bin/kmod").exists()
    {
        let _ = fs::create_dir_all("/run/tmpfiles.d");
        let r = run(
            "/usr/bin/kmod",
            &[
                "static-nodes",
                "--format=tmpfiles",
                "--output=/run/tmpfiles.d/static-nodes.conf",
            ],
        );
        if r != Some(0) {
            warn(&format!("kmod static-nodes exited {}", code(r)));
        }
    }
    tmpfiles(
        &["--prefix=/dev", "--create", "--boot", "--graceful"],
        "systemd-tmpfiles --prefix=/dev",
    );
    if Path::new("/sys/fs/selinux/enforce").exists() && has("restorecon") {
        // /run, but not what s6-linux-init and s6-rc-init made there before
        // us: init_t's type transitions gave them init_var_run_t, and
        // resetting them to the file_contexts default (var_run_t without a
        // filecon) is what kept needing fixes (s6-rc-update denied its
        // renames, the catch-all fifo, the scan directory).
        restorecon(&["-R", "/dev"]);
        restorecon(&["/run"]);
        if let Ok(rd) = fs::read_dir("/run") {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if keep_run_label(&n) {
                    continue;
                }
                restorecon(&["-R", &e.path().to_string_lossy()]);
            }
        }
    }
    machine_id();
}

/// What s6-linux-init and s6-rc-init made in /run before mount-api runs.
fn keep_run_label(name: &str) -> bool {
    matches!(name, "service" | "uncaught-logs" | "s6-rc")
        || name.starts_with("s6-rc:")
        || name.starts_with("s6-linux-init")
}

/// In place of systemd-machine-id-setup (rule 2): dbus's dbus-uuidgen
/// writes the same 32 hex digits when /etc/machine-id is missing or empty.
fn machine_id() {
    let id = "/etc/machine-id";
    if fs::metadata(id).map(|m| m.len() > 0).unwrap_or(false) {
        return;
    }
    if !quiet("dbus-uuidgen", &["--ensure=/etc/machine-id"]) {
        warn("dbus-uuidgen --ensure=/etc/machine-id failed");
    }
}

fn kernel_release() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn mount_fs() {
    // fsck of the root filesystem is not done here; the initramfs normally
    // handles it (rd.fsck / fsck.mode kernel options).
    // As systemd-remount-fs: the options fstab gives the root filesystem
    // (rw unless it says ro); "remount" with no device reads them from
    // fstab. Without an fstab line, read-write as before.
    if quiet("findmnt", &["--fstab", "-n", "/"]) {
        if !quiet("mount", &["-o", "remount", "/"]) {
            warn("remount of / with its fstab options failed");
        }
    } else {
        quiet("mount", &["-o", "remount,rw", "/"]);
    }
    if run("mount", &["-a", "-O", "no_netdev"]) != Some(0) {
        warn("mount -a: some filesystems did not mount");
    }
    quiet("swapon", &["-a"]);
}

fn umount_fs() {
    quiet("swapoff", &["-a"]);
    // SAFETY: sync takes no arguments.
    unsafe { libc::sync() };
}

fn udev_trigger() {
    run("udevadm", &["trigger", "--type=subsystems", "--action=add"]);
    run("udevadm", &["trigger", "--type=devices", "--action=add"]);
    if run("udevadm", &["settle", "--timeout=60"]) != Some(0) {
        warn("udevadm settle timed out");
    }
}

/// The modules to load: *.conf in the directories, /etc overriding /run
/// overriding /usr/lib by file name (as systemd-modules-load), files in name
/// order, comments (# or ;) and blank lines left out.
fn modules_to_load(dirs: &[&Path]) -> Vec<String> {
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".conf") {
                files.entry(n).or_insert_with(|| e.path());
            }
        }
    }
    let mut out = Vec::new();
    for p in files.values() {
        for l in fs::read_to_string(p).unwrap_or_default().lines() {
            let m = l.split(['#', ';']).next().unwrap_or("").trim();
            if !m.is_empty() {
                out.push(m.to_string());
            }
        }
    }
    out
}

fn modules_load() {
    let dirs = [
        Path::new("/etc/modules-load.d"),
        Path::new("/run/modules-load.d"),
        Path::new("/usr/lib/modules-load.d"),
    ];
    for m in modules_to_load(&dirs) {
        run("modprobe", &["-b", &m]);
    }
}

/// The first line of /etc/hostname that isn't a comment, else localhost.
fn hostname_from(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("localhost")
        .to_string()
}

fn hostname() {
    let h = hostname_from(&fs::read_to_string("/etc/hostname").unwrap_or_default());
    // SAFETY: the buffer and its length.
    if unsafe { libc::sethostname(h.as_ptr().cast(), h.len()) } != 0 {
        warn(&format!("sethostname {h} failed"));
    }
}

/// /etc/vconsole.conf's settings (KEY=VALUE, optionally quoted), with
/// vconsole.keymap=, vconsole.keymap_toggle=, vconsole.font=,
/// vconsole.font_map= and vconsole.font_unimap= on the kernel command line
/// taking precedence, as systemd-vconsole-setup reads them.
fn vconsole_settings(conf: &str, cmdline: &str) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    for l in conf.lines() {
        let l = l.trim();
        if l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            s.insert(k.trim().to_string(), v.to_string());
        }
    }
    for w in cmdline.split_whitespace() {
        let w = w.strip_prefix("rd.").unwrap_or(w);
        if let Some((k, v)) = w.strip_prefix("vconsole.").and_then(|r| r.split_once('=')) {
            s.insert(k.to_uppercase(), v.to_string());
        }
    }
    s.retain(|_, v| !v.is_empty());
    s
}

/// In place of systemd-vconsole-setup (rule 2): kbd's loadkeys and setfont
/// with /etc/vconsole.conf's keymap and font, the consoles in UTF-8 mode.
/// udev's 90-vconsole.rules asks systemd-vconsole-setup.service for each new
/// console; this runs once the boot's udev settle is done, for tty1-tty12.
fn vconsole() {
    if !Path::new("/dev/tty0").exists() {
        return;
    }
    let s = vconsole_settings(
        &fs::read_to_string("/etc/vconsole.conf").unwrap_or_default(),
        &cmdline(),
    );
    let ttys: Vec<String> = (1..=12)
        .map(|n| format!("/dev/tty{n}"))
        .filter(|t| Path::new(t).exists())
        .collect();
    let utf8 = has("kbd_mode");
    for t in &ttys {
        if utf8 {
            quiet("kbd_mode", &["-u", "-C", t]);
        }
        // ESC % G: the console's UTF-8 mode.
        let _ = fs::OpenOptions::new()
            .write(true)
            .open(t)
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"\x1b%G"));
    }
    if let Some(k) = s.get("KEYMAP") {
        let mut a = vec!["-q", "-C", "/dev/tty1", "-u", k.as_str()];
        if let Some(t) = s.get("KEYMAP_TOGGLE") {
            a.push(t);
        }
        if run("loadkeys", &a) != Some(0) {
            warn(&format!("loadkeys {k} failed"));
        }
    }
    if let Some(f) = s.get("FONT") {
        for t in &ttys {
            let mut a = vec!["-C", t.as_str(), f.as_str()];
            if let Some(m) = s.get("FONT_MAP") {
                a.extend(["-m", m]);
            }
            if let Some(u) = s.get("FONT_UNIMAP") {
                a.extend(["-u", u]);
            }
            if !quiet("setfont", &a) {
                warn(&format!("setfont {f} on {t} failed"));
                break;
            }
        }
    }
}

/// udev's ID_PATH of a sysfs device.
fn id_path(dev: &Path) -> Option<String> {
    let props = output(
        "udevadm",
        &["info", "-q", "property", "-p", &dev.to_string_lossy()],
    )?;
    props
        .lines()
        .find_map(|l| l.strip_prefix("ID_PATH="))
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn state_dir(dir: &str) {
    let _ = fs::create_dir_all(dir);
    restorecon(&[dir]);
}

/// In place of systemd-rfkill (socket-activated by udev for /dev/rfkill):
/// restore each radio's soft block at boot, save it at shutdown. The state
/// lives in systemd-rfkill's files (/var/lib/systemd/rfkill/ID_PATH:TYPE,
/// "0" or "1"), so the systemd boot entry shares it. Devices that appear
/// after the boot's udev settle keep their state.
fn rfkill(save: bool) {
    let dir = "/var/lib/systemd/rfkill";
    let Ok(rd) = fs::read_dir("/sys/class/rfkill") else {
        return;
    };
    if save {
        state_dir(dir);
    } else {
        if restore_state_off(&cmdline()) {
            return;
        }
        if !has("rfkill") {
            warn("rfkill (util-linux) not found");
            return;
        }
    }
    for e in rd.flatten() {
        let d = e.path();
        let Some(kind) = read_trim(&d.join("type")) else {
            continue;
        };
        let name = e.file_name().to_string_lossy().into_owned();
        let label = format!(
            "rfkill: {name} ({})",
            read_trim(&d.join("name")).unwrap_or_default()
        );
        // systemd-rfkill skips devices without ID_PATH too.
        let Some(path) = id_path(&d) else {
            warn(&format!("{label}: no ID_PATH from udev, skipped"));
            continue;
        };
        let f = Path::new(dir).join(format!("{path}:{kind}"));
        let idx = name.trim_start_matches("rfkill");
        if save {
            let soft = read_trim(&d.join("soft")).unwrap_or_default();
            if fs::write(&f, format!("{soft}\n")).is_err() {
                warn(&format!("saving {} failed", f.display()));
            } else {
                warn(&format!("{label}: saved {soft} to {}", f.display()));
            }
        } else {
            let op = match read_trim(&f).as_deref() {
                Some("1") => "block",
                Some("0") => "unblock",
                _ => {
                    warn(&format!("{label}: no saved state in {}", f.display()));
                    continue;
                }
            };
            if run("rfkill", &[op, idx]) != Some(0) {
                warn(&format!("rfkill {op} {name}"));
            } else {
                let soft = read_trim(&d.join("soft")).unwrap_or_default();
                warn(&format!(
                    "{label}: {op} from {}, soft now {soft}",
                    f.display()
                ));
            }
        }
    }
}

/// systemd-backlight's file name for a device: ID_PATH (or nothing),
/// subsystem and name, as in /var/lib/systemd/backlight.
fn backlight_file(id_path: Option<&str>, subsystem: &str, name: &str) -> String {
    match id_path {
        Some(p) => format!("{p}:{subsystem}:{name}"),
        None => format!("{subsystem}:{name}"),
    }
}

/// The brightness to restore: the saved value within 0..=max, and, for a
/// screen, at least 5% of max (as systemd-backlight, so a saved 0 doesn't
/// leave the screen black).
fn backlight_value(saved: u64, max: u64, screen: bool) -> u64 {
    let v = saved.min(max);
    if screen {
        v.max(max.div_ceil(20).max(1))
    } else {
        v
    }
}

/// In place of systemd-backlight@.service (rule 2): restore each screen
/// backlight and keyboard light at boot and save it at shutdown, in
/// systemd-backlight's files (/var/lib/systemd/backlight), so the systemd
/// boot entry shares them.
fn backlight(save: bool) {
    let dir = "/var/lib/systemd/backlight";
    if save {
        state_dir(dir);
    } else if restore_state_off(&cmdline()) {
        return;
    }
    for (subsystem, filter) in [("backlight", None), ("leds", Some("::kbd_backlight"))] {
        let Ok(rd) = fs::read_dir(format!("/sys/class/{subsystem}")) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if filter.is_some_and(|f| !name.contains(f)) {
                continue;
            }
            let d = e.path();
            let f = Path::new(dir).join(backlight_file(id_path(&d).as_deref(), subsystem, &name));
            if save {
                let cur = read_trim(&d.join("brightness")).unwrap_or_default();
                if !cur.is_empty() && fs::write(&f, format!("{cur}\n")).is_err() {
                    warn(&format!("saving {} failed", f.display()));
                }
                continue;
            }
            let (Some(saved), Some(max)) = (
                read_trim(&f).and_then(|s| s.parse::<u64>().ok()),
                read_trim(&d.join("max_brightness")).and_then(|s| s.parse::<u64>().ok()),
            ) else {
                continue;
            };
            let v = backlight_value(saved, max, subsystem == "backlight");
            if fs::write(d.join("brightness"), v.to_string()).is_err() {
                warn(&format!("restoring the brightness of {name} failed"));
            }
        }
    }
}

const NOLOGIN: &str = "/run/nologin";

/// In place of systemd-user-sessions.service: unprivileged logins are
/// allowed once tmpfiles has run (it creates /run/nologin at boot,
/// tmpfiles.d/systemd-nologin.conf) and refused again when the shutdown
/// starts (rc.shutdown runs `early user-sessions-stop`; pam_nologin reads
/// the file).
pub(crate) fn user_sessions(stop: bool) {
    if stop {
        let msg = "System is going down, unprivileged users are not permitted to log in anymore. For technical details, see pam_nologin(8).\n";
        if fs::write(NOLOGIN, msg).is_err() {
            warn("can't create /run/nologin");
        }
    } else if let Err(e) = fs::remove_file(NOLOGIN) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn(&format!("can't remove /run/nologin: {e}"));
        }
    }
}

fn tmpfiles_setup() {
    // Like systemd-tmpfiles-setup.service: SuccessExitStatus=DATAERR CANTCREAT
    tmpfiles(
        &["--create", "--remove", "--boot", "--exclude-prefix=/dev"],
        "systemd-tmpfiles",
    );
    // tmpfiles.d/systemd-nologin.conf just made /run/nologin; logins open
    // here, in place of systemd-user-sessions (no service of its own: one
    // that units depend on would be new at an update, and s6-rc-update
    // restarts what gains a dependency on a new service: gdm with the
    // session, work_history part 220). rc.shutdown writes it again.
    user_sessions(false);
}

fn sysusers() {
    let r = run("/usr/bin/systemd-sysusers", &[]);
    if r != Some(0) {
        warn(&format!("systemd-sysusers exited {}", code(r)));
    }
}

fn sysctl() {
    let r = run("/usr/sbin/sysctl", &["-q", "--system"]);
    if r != Some(0) {
        warn(&format!("sysctl --system exited {}", code(r)));
    }
}

const NO_RESOLVED_CONF: &str =
    "# Written at boot by uinitconverter's \"resolv\" s6 service (RESOLVED=no).
# rc-manager=symlink: Fedora's resolvconf is resolvectl, i.e. resolved.
[main]
dns=default
rc-manager=symlink
";

/// RESOLVED=no (initconvert init --resolved=no): systemd-resolved does not
/// run on this boot. NetworkManager handles DNS itself, and the stub file
/// /etc/resolv.conf links to points at NetworkManager's copy. All of it is
/// in /run, so the systemd boot keeps using resolved.
fn resolv() {
    let nm = "/run/NetworkManager";
    let _ = fs::create_dir_all(format!("{nm}/conf.d"));
    let conf = format!("{nm}/conf.d/99-uinitconverter-no-resolved.conf");
    if fs::write(&conf, NO_RESOLVED_CONF).is_err() {
        warn(&format!("can't write {conf}"));
    }
    if let Some(t) = output("readlink", &["-m", "/etc/resolv.conf"]) {
        let t = t.trim();
        if t.starts_with("/run/systemd/resolve/") {
            if let Some(parent) = Path::new(t).parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::remove_file(t);
            if symlink(format!("{nm}/resolv.conf"), t).is_err() {
                warn(&format!("can't link {t}"));
            }
        }
    }
    restorecon(&["-R", nm, "/run/systemd/resolve"]);
}

/// A zram-generator swap unit's What=, Priority= and Options=.
fn swap_unit(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    (get("What="), get("Priority="), get("Options="))
}

/// In place of zram-generator, a systemd generator (nothing runs the
/// generators on the s6 boot): Fedora's compressed swap. The generator
/// itself decides, as at a systemd boot, which /dev/zramN to make from
/// zram-generator.conf (zram-generator-defaults: zram0, min(ram, 8192)),
/// the memory and systemd.zram= on the kernel command line, loads the
/// module and adds the devices. Its dev-zramN.swap units go where systemd
/// puts them (/run/systemd/generator, the one place its SELinux domain may
/// write) and are read here only; they are not in the unit path. Each
/// device is then set up as systemd-zram-setup@zramN does and swapped on
/// with the unit's priority and options.
fn zram(stop: bool) {
    let zg = "/usr/lib/systemd/system-generators/zram-generator";
    if !Path::new(zg).exists() {
        return;
    }
    let out = "/run/systemd/generator";
    if !stop {
        let early = format!("{out}.early");
        let late = format!("{out}.late");
        for d in [out, &early, &late] {
            let _ = fs::create_dir_all(d);
        }
        restorecon(&[out, &early, &late]);
        let r = run(zg, &[out, &early, &late]);
        if r != Some(0) {
            warn(&format!("zram-generator exited {}", code(r)));
            return;
        }
    }
    let Ok(rd) = fs::read_dir(out) else { return };
    let mut units: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy();
            n.starts_with("dev-zram") && n.ends_with(".swap")
        })
        .collect();
    units.sort();
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    for u in units {
        let (Some(dev), prio, opts) = swap_unit(&fs::read_to_string(&u).unwrap_or_default()) else {
            continue;
        };
        let n = dev.rsplit('/').next().unwrap_or("").to_string();
        if n.is_empty() {
            continue;
        }
        if !stop {
            let r = run(zg, &["--setup-device", &n]);
            if r != Some(0) {
                warn(&format!(
                    "zram-generator --setup-device {n} exited {}",
                    code(r)
                ));
                continue;
            }
            let mut a: Vec<&str> = Vec::new();
            if let Some(p) = &prio {
                a.extend(["-p", p]);
            }
            if let Some(o) = &opts {
                a.extend(["-o", o]);
            }
            a.push(&dev);
            if run("swapon", &a) != Some(0) {
                warn(&format!("swapon {dev} failed"));
            }
        } else {
            if swaps
                .lines()
                .any(|l| l.split_whitespace().next() == Some(dev.as_str()))
                && run("swapoff", &[&dev]) != Some(0)
            {
                warn(&format!("swapoff {dev} failed"));
            }
            let r = run(zg, &["--reset-device", &n]);
            if r != Some(0) {
                warn(&format!(
                    "zram-generator --reset-device {n} exited {}",
                    code(r)
                ));
            }
        }
    }
}

/// `early STEP`: run one step; always exit 0 (see the module doc), except
/// for an unknown step.
pub fn main(args: &[String]) -> ! {
    match args.first().map(String::as_str).unwrap_or("") {
        "mount-api" => mount_api(),
        "mount-fs" => mount_fs(),
        "umount-fs" => umount_fs(),
        "udev-trigger" => udev_trigger(),
        "modules-load" => modules_load(),
        "hostname" => hostname(),
        "tmpfiles" => tmpfiles_setup(),
        "sysusers" => sysusers(),
        "sysctl" => sysctl(),
        "vconsole" => vconsole(),
        "rfkill" => rfkill(false),
        "rfkill-save" => rfkill(true),
        "backlight" => backlight(false),
        "backlight-save" => backlight(true),
        "user-sessions" => user_sessions(false),
        "user-sessions-stop" => user_sessions(true),
        "resolv" => resolv(),
        "zram" => zram(false),
        "zram-stop" => zram(true),
        _ => {
            eprintln!("{USAGE}");
            exit(2);
        }
    }
    exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "uinitconverter-early-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn modules_load_order_and_overrides() {
        let t = tmp("modules");
        let (etc, usr) = (t.join("etc"), t.join("usr"));
        fs::create_dir_all(&etc).unwrap();
        fs::create_dir_all(&usr).unwrap();
        fs::write(usr.join("a.conf"), "tun\n# comment\n\nloop ; trailing\n").unwrap();
        fs::write(usr.join("b.conf"), "vhost_net\n").unwrap();
        // /etc's b.conf hides /usr/lib's.
        fs::write(etc.join("b.conf"), "uinput\n").unwrap();
        fs::write(etc.join("c.txt"), "ignored\n").unwrap();
        assert_eq!(modules_to_load(&[&etc, &usr]), ["tun", "loop", "uinput"]);
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn hostname_first_line() {
        assert_eq!(hostname_from("# c\n\nbox\nother\n"), "box");
        assert_eq!(hostname_from(""), "localhost");
    }

    #[test]
    fn vconsole_conf_and_cmdline() {
        let s = vconsole_settings(
            "KEYMAP=us\nFONT=\"eurlatgr\"\n# FONT_MAP=x\nKEYMAP_TOGGLE=\n",
            "quiet vconsole.keymap=de rd.vconsole.font_unimap=uni",
        );
        assert_eq!(s.get("KEYMAP").map(String::as_str), Some("de"));
        assert_eq!(s.get("FONT").map(String::as_str), Some("eurlatgr"));
        assert_eq!(s.get("FONT_UNIMAP").map(String::as_str), Some("uni"));
        assert!(!s.contains_key("FONT_MAP"));
        assert!(!s.contains_key("KEYMAP_TOGGLE"));
    }

    #[test]
    fn backlight_names_and_values() {
        assert_eq!(
            backlight_file(Some("pci-0000:00:02.0"), "backlight", "intel_backlight"),
            "pci-0000:00:02.0:backlight:intel_backlight"
        );
        assert_eq!(
            backlight_file(None, "leds", "tpacpi::kbd_backlight"),
            "leds:tpacpi::kbd_backlight"
        );
        // A screen never comes back black; within max either way.
        assert_eq!(backlight_value(0, 1000, true), 50);
        assert_eq!(backlight_value(700, 1000, true), 700);
        assert_eq!(backlight_value(5000, 1000, true), 1000);
        assert_eq!(backlight_value(0, 2, false), 0);
        assert_eq!(backlight_value(0, 7, true), 1);
    }

    #[test]
    fn zram_swap_unit_fields() {
        let (w, p, o) = swap_unit("[Swap]\nWhat=/dev/zram0\nPriority=100\nOptions=discard\n");
        assert_eq!(w.as_deref(), Some("/dev/zram0"));
        assert_eq!(p.as_deref(), Some("100"));
        assert_eq!(o.as_deref(), Some("discard"));
        assert_eq!(swap_unit("What=/dev/zram1\n").1, None);
    }

    #[test]
    fn run_labels_kept() {
        for n in [
            "service",
            "uncaught-logs",
            "s6-rc",
            "s6-rc:x",
            "s6-linux-init-shutdownd",
        ] {
            assert!(keep_run_label(n), "{n}");
        }
        assert!(!keep_run_label("NetworkManager"));
    }

    #[test]
    fn restore_state_option() {
        assert!(restore_state_off("ro systemd.restore_state=0 quiet"));
        assert!(!restore_state_off("ro systemd.restore_state=1"));
    }
}
