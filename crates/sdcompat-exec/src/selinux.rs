//! The SELinux labelling systemd does as PID 1, with libselinux loaded at run
//! time: directories and socket files get their file_contexts label, and
//! sockets of socket units are created with the label of the service that
//! will use them. Only as root with SELinux enabled; otherwise every
//! function does nothing.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::OnceLock;

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
