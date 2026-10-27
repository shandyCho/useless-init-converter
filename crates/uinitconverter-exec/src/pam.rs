//! PAMName= support, loaded with dlopen() so there is no build-time libpam
//! dependency. Mirrors systemd: open the session in the supervisor process,
//! fork, run the service in the child, close the session when it exits.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

const PAM_SUCCESS: c_int = 0;
const PAM_CONV_ERR: c_int = 19;
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_SILENT: c_int = 0x8000;
const PAM_ESTABLISH_CRED: c_int = 0x2;
const PAM_DELETE_CRED: c_int = 0x4;
const PAM_TTY: c_int = 3;

#[repr(C)]
struct PamConv {
    conv: extern "C" fn(c_int, *mut *const c_void, *mut *mut c_void, *mut c_void) -> c_int,
    appdata: *mut c_void,
}

/// No interactive conversation is possible for a service.
extern "C" fn no_conv(_n: c_int, _msg: *mut *const c_void, _resp: *mut *mut c_void, _data: *mut c_void) -> c_int {
    PAM_CONV_ERR
}

type StartFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type FlagsFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type SetItemFn = unsafe extern "C" fn(*mut c_void, c_int, *const c_void) -> c_int;
type EnvFn = unsafe extern "C" fn(*mut c_void) -> *mut *mut c_char;
type StrErrFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;

struct Lib {
    start: StartFn,
    end: FlagsFn,
    set_item: SetItemFn,
    acct_mgmt: FlagsFn,
    setcred: FlagsFn,
    open_session: FlagsFn,
    close_session: FlagsFn,
    getenvlist: EnvFn,
    strerror: StrErrFn,
}

fn load() -> Result<Lib, String> {
    unsafe {
        let h = libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW);
        if h.is_null() {
            return Err("cannot load libpam.so.0".into());
        }
        let sym = |name: &CStr| -> Result<*mut c_void, String> {
            let p = libc::dlsym(h, name.as_ptr());
            if p.is_null() {
                Err(format!("libpam: missing {}", name.to_string_lossy()))
            } else {
                Ok(p)
            }
        };
        Ok(Lib {
            start: std::mem::transmute::<*mut c_void, StartFn>(sym(c"pam_start")?),
            end: std::mem::transmute::<*mut c_void, FlagsFn>(sym(c"pam_end")?),
            set_item: std::mem::transmute::<*mut c_void, SetItemFn>(sym(c"pam_set_item")?),
            acct_mgmt: std::mem::transmute::<*mut c_void, FlagsFn>(sym(c"pam_acct_mgmt")?),
            setcred: std::mem::transmute::<*mut c_void, FlagsFn>(sym(c"pam_setcred")?),
            open_session: std::mem::transmute::<*mut c_void, FlagsFn>(sym(c"pam_open_session")?),
            close_session: std::mem::transmute::<*mut c_void, FlagsFn>(sym(c"pam_close_session")?),
            getenvlist: std::mem::transmute::<*mut c_void, EnvFn>(sym(c"pam_getenvlist")?),
            strerror: std::mem::transmute::<*mut c_void, StrErrFn>(sym(c"pam_strerror")?),
        })
    }
}

pub struct Session {
    lib: Lib,
    handle: *mut c_void,
    conv: Box<PamConv>,
}

impl Session {
    fn err(&self, what: &str, r: c_int) -> String {
        let msg = unsafe { CStr::from_ptr((self.lib.strerror)(self.handle, r)) };
        format!("{what}: {}", msg.to_string_lossy())
    }

    /// pam_start + acct_mgmt + setcred + open_session. Returns the session and
    /// the environment PAM modules exported.
    pub fn open(service: &str, user: &str, tty: Option<&str>) -> Result<(Session, Vec<(String, String)>), String> {
        let lib = load()?;
        let conv = Box::new(PamConv { conv: no_conv, appdata: std::ptr::null_mut() });
        let svc = CString::new(service).map_err(|e| e.to_string())?;
        let usr = CString::new(user).map_err(|e| e.to_string())?;
        let mut handle: *mut c_void = std::ptr::null_mut();
        let r = unsafe { (lib.start)(svc.as_ptr(), usr.as_ptr(), &*conv, &mut handle) };
        if r != PAM_SUCCESS {
            return Err(format!("pam_start({service}) failed: {r}"));
        }
        let s = Session { lib, handle, conv };
        if let Some(t) = tty {
            let ct = CString::new(t).map_err(|e| e.to_string())?;
            unsafe { (s.lib.set_item)(s.handle, PAM_TTY, ct.as_ptr() as *const c_void) };
        }
        unsafe {
            let r = (s.lib.acct_mgmt)(s.handle, PAM_SILENT);
            if r != PAM_SUCCESS && r != PAM_NEW_AUTHTOK_REQD {
                let e = s.err("pam_acct_mgmt", r);
                (s.lib.end)(s.handle, r);
                return Err(e);
            }
            // Not fatal, as in systemd: stacks without auth lines (Fedora's
            // systemd-user) fall back to "other", whose pam_deny fails here.
            (s.lib.setcred)(s.handle, PAM_ESTABLISH_CRED | PAM_SILENT);
            let r = (s.lib.open_session)(s.handle, PAM_SILENT);
            if r != PAM_SUCCESS {
                let e = s.err("pam_open_session", r);
                (s.lib.setcred)(s.handle, PAM_DELETE_CRED | PAM_SILENT);
                (s.lib.end)(s.handle, r);
                return Err(e);
            }
        }
        let mut env = Vec::new();
        unsafe {
            let list = (s.lib.getenvlist)(s.handle);
            if !list.is_null() {
                let mut i = 0;
                loop {
                    let p = *list.add(i);
                    if p.is_null() {
                        break;
                    }
                    let kv = CStr::from_ptr(p).to_string_lossy().into_owned();
                    if let Some((k, v)) = kv.split_once('=') {
                        env.push((k.to_string(), v.to_string()));
                    }
                    libc::free(p as *mut c_void);
                    i += 1;
                }
                libc::free(list as *mut c_void);
            }
        }
        Ok((s, env))
    }

    pub fn close(self) {
        unsafe {
            (self.lib.close_session)(self.handle, PAM_SILENT);
            (self.lib.setcred)(self.handle, PAM_DELETE_CRED | PAM_SILENT);
            (self.lib.end)(self.handle, PAM_SUCCESS);
        }
        drop(self.conv);
    }
}
