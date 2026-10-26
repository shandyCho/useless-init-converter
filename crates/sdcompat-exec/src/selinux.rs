//! The SELinux labelling systemd does as PID 1, with libselinux loaded at run
//! time: directories and socket files get their file_contexts label, and
//! sockets of socket units are created with the label of the service that
//! will use them. And the access checks systemd makes on D-Bus callers
//! (selinux-access.c). Only as root with SELinux enabled; otherwise every
//! function does nothing.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

struct Lib {
    restorecon: unsafe extern "C" fn(*const c_char, c_uint) -> c_int,
    getcon: unsafe extern "C" fn(*mut *mut c_char) -> c_int,
    getfilecon: unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int,
    string_to_security_class: unsafe extern "C" fn(*const c_char) -> u16,
    compute_create: unsafe extern "C" fn(*const c_char, *const c_char, u16, *mut *mut c_char) -> c_int,
    setsockcreatecon: unsafe extern "C" fn(*const c_char) -> c_int,
    freecon: unsafe extern "C" fn(*mut c_char),
    selabel_open: unsafe extern "C" fn(c_uint, *const c_void, c_uint) -> *mut c_void,
    selabel_lookup: unsafe extern "C" fn(*mut c_void, *mut *mut c_char, *const c_char, c_int) -> c_int,
    selabel_close: unsafe extern "C" fn(*mut c_void),
    lsetfilecon: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int,
    getcon_raw: unsafe extern "C" fn(*mut *mut c_char) -> c_int,
    getfilecon_raw: unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int,
    check_access: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char, *mut c_void) -> c_int,
    getenforce: unsafe extern "C" fn() -> c_int,
}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| if unsafe { libc::geteuid() } == 0 { open() } else { None })
        .as_ref()
}

fn open() -> Option<Lib> {
    let h = unsafe { libc::dlopen(c"libselinux.so.1".as_ptr(), libc::RTLD_NOW) };
    if h.is_null() {
        return None;
    }
    let sym = |name: &CStr| {
        let p = unsafe { libc::dlsym(h, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    };
    let enabled: unsafe extern "C" fn() -> c_int = unsafe { std::mem::transmute(sym(c"is_selinux_enabled")?) };
    if unsafe { enabled() } <= 0 {
        return None;
    }
    // SAFETY: the signatures match libselinux's <selinux/selinux.h>.
    unsafe {
        Some(Lib {
            restorecon: std::mem::transmute::<*mut c_void, _>(sym(c"selinux_restorecon")?),
            getcon: std::mem::transmute::<*mut c_void, _>(sym(c"getcon")?),
            getfilecon: std::mem::transmute::<*mut c_void, _>(sym(c"getfilecon")?),
            string_to_security_class: std::mem::transmute::<*mut c_void, _>(sym(c"string_to_security_class")?),
            compute_create: std::mem::transmute::<*mut c_void, _>(sym(c"security_compute_create")?),
            setsockcreatecon: std::mem::transmute::<*mut c_void, _>(sym(c"setsockcreatecon")?),
            freecon: std::mem::transmute::<*mut c_void, _>(sym(c"freecon")?),
            selabel_open: std::mem::transmute::<*mut c_void, _>(sym(c"selabel_open")?),
            selabel_lookup: std::mem::transmute::<*mut c_void, _>(sym(c"selabel_lookup")?),
            selabel_close: std::mem::transmute::<*mut c_void, _>(sym(c"selabel_close")?),
            lsetfilecon: std::mem::transmute::<*mut c_void, _>(sym(c"lsetfilecon")?),
            getcon_raw: std::mem::transmute::<*mut c_void, _>(sym(c"getcon_raw")?),
            getfilecon_raw: std::mem::transmute::<*mut c_void, _>(sym(c"getfilecon_raw")?),
            check_access: std::mem::transmute::<*mut c_void, _>(sym(c"selinux_check_access")?),
            getenforce: std::mem::transmute::<*mut c_void, _>(sym(c"security_getenforce")?),
        })
    }
}

fn take(lib: &Lib, p: *mut c_char) -> String {
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { (lib.freecon)(p) };
    s
}

/// Give PATH its label from file_contexts (systemd's mkdir_label and
/// mac_selinux_create_file_prepare). Errors are ignored, as a wrong label
/// is only a problem under enforcing and then shows up as a denial.
pub fn restorecon(path: &str) {
    let Some(lib) = lib() else { return };
    let Ok(c) = CString::new(path) else { return };
    unsafe { (lib.restorecon)(c.as_ptr(), 0) };
}

/// Give each PATH the file_contexts label of AS, the path it will appear at
/// (systemd's label_fix_full and symlink_label, used for the private /dev
/// built in a staging tmpfs). Symlinks are labelled themselves. Errors are
/// ignored, as in restorecon().
pub fn label_as(pairs: &[(String, String)]) {
    if let Some(lib) = lib() {
        label_as_with(lib, pairs);
    }
}

fn label_as_with(lib: &Lib, pairs: &[(String, String)]) {
    // SELABEL_CTX_FILE = 0, no options.
    let h = unsafe { (lib.selabel_open)(0, std::ptr::null(), 0) };
    if h.is_null() {
        return;
    }
    for (path, as_path) in pairs {
        let (Ok(p), Ok(a)) = (CString::new(path.as_str()), CString::new(as_path.as_str())) else { continue };
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::lstat(p.as_ptr(), &mut st) } < 0 {
            continue;
        }
        let mut con = std::ptr::null_mut();
        if unsafe { (lib.selabel_lookup)(h, &mut con, a.as_ptr(), (st.st_mode & libc::S_IFMT) as c_int) } < 0 {
            continue;
        }
        unsafe {
            (lib.lsetfilecon)(p.as_ptr(), con);
            (lib.freecon)(con);
        }
    }
    unsafe { (lib.selabel_close)(h) };
}

/// The domain a process of ours would enter by executing EXE
/// (systemd's mac_selinux_get_create_label_from_exe).
pub fn exec_label(exe: &str) -> Option<String> {
    exec_label_with(lib()?, exe)
}

fn exec_label_with(lib: &Lib, exe: &str) -> Option<String> {
    let c = CString::new(exe).ok()?;
    let (mut mine, mut file, mut new) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    unsafe {
        if (lib.getcon)(&mut mine) < 0 {
            return None;
        }
        let mine = take(lib, mine);
        if (lib.getfilecon)(c.as_ptr(), &mut file) < 0 {
            return None;
        }
        let file = take(lib, file);
        let class = (lib.string_to_security_class)(c"process".as_ptr());
        let (m, f) = (CString::new(mine).ok()?, CString::new(file).ok()?);
        if class == 0 || (lib.compute_create)(m.as_ptr(), f.as_ptr(), class, &mut new) < 0 {
            return None;
        }
        Some(take(lib, new))
    }
}

/// Label for sockets this thread creates from now on; None restores the default.
pub fn set_socket_label(label: Option<&str>) {
    let Some(lib) = lib() else { return };
    let c = label.and_then(|l| CString::new(l).ok());
    unsafe { (lib.setsockcreatecon)(c.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null())) };
}

/// Whether access checks are made: as root with SELinux enabled.
pub fn access_checks() -> bool {
    lib().is_some()
}

/// Enforcing mode (security_getenforce); false without SELinux.
pub fn enforcing() -> bool {
    lib().is_some_and(|lib| unsafe { (lib.getenforce)() } == 1)
}

/// Our own context, raw (getcon_raw), the target of the "system" class
/// checks, as systemd's get_our_contexts().
pub fn own_context() -> Option<String> {
    static OWN: OnceLock<Option<String>> = OnceLock::new();
    OWN.get_or_init(|| {
        let lib = lib()?;
        let mut p = std::ptr::null_mut();
        (unsafe { (lib.getcon_raw)(&mut p) } >= 0 && !p.is_null()).then(|| take(lib, p))
    })
    .clone()
}

/// The raw context of a file, following symlinks (systemd reads the unit
/// file's from the open file, so the target's).
pub fn file_context(path: &str) -> Option<String> {
    let lib = lib()?;
    let c = CString::new(path).ok()?;
    let mut p = std::ptr::null_mut();
    (unsafe { (lib.getfilecon_raw)(c.as_ptr(), &mut p) } >= 0 && !p.is_null()).then(|| take(lib, p))
}

/// selinux_check_access(SCON, TCON, TCLASS, PERM): Ok when the policy allows
/// it, or when it doesn't but SELinux or the caller's domain is permissive
/// (the userspace AVC answers so and logs the denial). Err carries errno's
/// text. libselinux's AVC takes no locks of its own here, so calls are
/// serialised.
pub fn check_access(scon: &str, tcon: &str, tclass: &str, perm: &str) -> Result<(), String> {
    static LOCK: Mutex<()> = Mutex::new(());
    let Some(lib) = lib() else { return Ok(()) };
    let args = (CString::new(scon), CString::new(tcon), CString::new(tclass), CString::new(perm));
    let (Ok(s), Ok(t), Ok(c), Ok(p)) = args else {
        return Err("invalid context".into());
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    check_access_with(lib, &s, &t, &c, &p)
}

fn check_access_with(lib: &Lib, s: &CStr, t: &CStr, c: &CStr, p: &CStr) -> Result<(), String> {
    if unsafe { (lib.check_access)(s.as_ptr(), t.as_ptr(), c.as_ptr(), p.as_ptr(), std::ptr::null_mut()) } < 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.raw_os_error().unwrap_or(0) == 0 { "Permission denied".into() } else { e.to_string() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks the libselinux bindings where SELinux is enabled (any user).
    #[test]
    fn exec_label_on_this_host() {
        let Some(lib) = open() else {
            eprintln!("SELinux not enabled, skipping");
            return;
        };
        let label = exec_label_with(&lib, "/bin/sh").expect("security_compute_create");
        assert_eq!(label.split(':').count() >= 4, true, "{label}");
        assert_eq!(exec_label_with(&lib, "/nonexistent"), None);
    }

    /// selinux_check_access as systemd calls it, on this host's policy: an
    /// unconfined caller may query the manager, a made-up domain may not.
    #[test]
    fn check_access_on_this_host() {
        let Some(lib) = open() else {
            eprintln!("SELinux not enabled, skipping");
            return;
        };
        let init = c"system_u:system_r:init_t:s0";
        let me = c"unconfined_u:unconfined_r:unconfined_t:s0-s0:c0.c1023";
        if check_access_with(&lib, me, init, c"system", c"status").is_err() {
            eprintln!("no unconfined_t here, skipping");
            return;
        }
        assert!(check_access_with(&lib, me, init, c"system", c"reload").is_ok());
        let unit = c"system_u:object_r:systemd_unit_file_t:s0";
        assert!(check_access_with(&lib, me, unit, c"service", c"start").is_ok());
        // chronyd_t may not reboot the machine.
        let chronyd = c"system_u:system_r:chronyd_t:s0";
        let denied = check_access_with(&lib, chronyd, init, c"system", c"reboot");
        if unsafe { (lib.getenforce)() } == 1 {
            assert!(denied.is_err(), "chronyd_t reboot allowed");
        }
    }

    /// label_as on a scratch directory and symlink (any user may relabel
    /// their own files where the policy leaves them unconfined).
    #[test]
    fn label_as_on_this_host() {
        let Some(lib) = open() else {
            eprintln!("SELinux not enabled, skipping");
            return;
        };
        let dir = std::env::temp_dir().join(format!("sdcompat-label-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let link = dir.join("fd");
        std::os::unix::fs::symlink("/proc/self/fd", &link).unwrap();
        let (d, l) = (dir.display().to_string(), link.display().to_string());
        label_as_with(&lib, &[(d.clone(), "/dev".into()), (l.clone(), "/dev/fd".into())]);
        let (dl, ll) = (xattr(&d), xattr(&l));
        std::fs::remove_dir_all(&dir).unwrap();
        if dl.contains("tmp") && ll.contains("tmp") {
            eprintln!("relabel refused here ({dl}), skipping");
            return;
        }
        assert!(dl.contains(":device_t:"), "{dl}");
        assert!(ll.contains(":device_t:"), "{ll}");
    }

    /// The security.selinux xattr of P itself (not following a symlink).
    fn xattr(p: &str) -> String {
        let c = CString::new(p).unwrap();
        let mut buf = [0u8; 256];
        let n = unsafe { libc::lgetxattr(c.as_ptr(), c"security.selinux".as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        assert!(n > 0, "lgetxattr {p}");
        String::from_utf8_lossy(&buf[..n as usize]).trim_end_matches('\0').to_string()
    }
}
