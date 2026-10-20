//! cgroup BPF settings: DevicePolicy=/DeviceAllow= (a BPF_PROG_TYPE_CGROUP_DEVICE
//! program) and IPAddressAllow=/IPAddressDeny= (BPF_PROG_TYPE_CGROUP_SKB on
//! ingress and egress). As in systemd's bpf-devices.c and bpf-firewall.c the
//! programs are assembled by hand, without libbpf. Unlike bpf-firewall.c the
//! address lists are compiled into the program instead of LPM-trie maps:
//! units list a handful of prefixes. The runner attaches the programs to the
//! unit's cgroup (BPF_F_ALLOW_MULTI) before anything runs there; they stay
//! attached until the next start replaces them. Programs carry systemd's
//! names (sd_devices, sd_fw_ingress, sd_fw_egress), which is also how a
//! start finds the previous start's programs to detach.

use std::ffi::CString;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

use crate::loader::Unit;

// Opcodes (linux/bpf_common.h, linux/bpf.h). ALU is 32-bit (zero-extends),
// ALU64 64-bit; JMP32 compares the low 32 bits.
const LDX_MEM_W: u8 = 0x61; // dst = *(u32 *)(src + off)
const ALU64_MOV_X: u8 = 0xbf;
const ALU64_MOV_K: u8 = 0xb7;
const ALU64_ADD_K: u8 = 0x07;
const ALU_MOV_X: u8 = 0xbc;
const ALU_AND_K: u8 = 0x54;
const ALU_RSH_K: u8 = 0x74;
const JMP_JA: u8 = 0x05;
const JMP_JEQ_K: u8 = 0x15;
const JMP_JNE_K: u8 = 0x55;
const JMP32_JNE_K: u8 = 0x56;
const JMP_CALL: u8 = 0x85;
const JMP_EXIT: u8 = 0x95;

const FUNC_SKB_LOAD_BYTES: i32 = 26;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Insn {
    pub code: u8,
    pub dst: u8,
    pub src: u8,
    pub off: i16,
    pub imm: i32,
}

impl Insn {
    fn encode(&self) -> [u8; 8] {
        // struct bpf_insn's dst_reg:4/src_reg:4 bitfield follows byte order.
        let regs = if cfg!(target_endian = "little") { self.dst | (self.src << 4) } else { (self.dst << 4) | self.src };
        let mut b = [0u8; 8];
        b[0] = self.code;
        b[1] = regs;
        b[2..4].copy_from_slice(&self.off.to_ne_bytes());
        b[4..8].copy_from_slice(&self.imm.to_ne_bytes());
        b
    }
}

/// Instructions with forward jumps to labels.
#[derive(Default)]
struct Asm {
    insns: Vec<Insn>,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, usize)>,
}

impl Asm {
    fn op(&mut self, code: u8, dst: u8, src: u8, off: i16, imm: i32) {
        self.insns.push(Insn { code, dst, src, off, imm });
    }

    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }

    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.insns.len());
    }

    fn jump(&mut self, code: u8, dst: u8, imm: i32, l: usize) {
        self.fixups.push((self.insns.len(), l));
        self.op(code, dst, 0, 0, imm);
    }

    fn ret(&mut self, v: i32) {
        self.op(ALU64_MOV_K, 0, 0, 0, v);
        self.op(JMP_EXIT, 0, 0, 0, 0);
    }

    fn finish(mut self) -> Vec<Insn> {
        for (at, l) in self.fixups {
            let target = self.labels[l].expect("unbound label");
            self.insns[at].off = (target as isize - at as isize - 1) as i16;
        }
        self.insns
    }
}

// --- DevicePolicy= / DeviceAllow= ---

pub const DEV_BLOCK: u32 = 1;
pub const DEV_CHAR: u32 = 2;
pub const ACC_MKNOD: u32 = 1;
pub const ACC_READ: u32 = 2;
pub const ACC_WRITE: u32 = 4;
const ACC_ALL: u32 = ACC_MKNOD | ACC_READ | ACC_WRITE;

/// One allowed device: a type, and a major/minor unless any matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevRule {
    pub ty: u32,
    pub major: Option<u32>,
    pub minor: Option<u32>,
    pub access: u32,
}

/// What DevicePolicy=closed (and auto with a DeviceAllow= list) adds, as
/// systemd's cgroup_apply_devices().
const AUTO_DEVICES: &[&str] = &[
    "/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", "/dev/ptmx",
    "/run/systemd/inaccessible/chr", "/run/systemd/inaccessible/blk",
];

fn parse_access(s: &str) -> Option<u32> {
    let mut acc = 0;
    for c in s.chars() {
        acc |= match c {
            'r' => ACC_READ,
            'w' => ACC_WRITE,
            'm' => ACC_MKNOD,
            _ => return None,
        };
    }
    (acc != 0).then_some(acc)
}

fn node_rule(path: &str, access: u32) -> io::Result<DevRule> {
    let md = fs::metadata(path)?;
    let ty = if md.file_type().is_char_device() {
        DEV_CHAR
    } else if md.file_type().is_block_device() {
        DEV_BLOCK
    } else {
        return Err(io::Error::other("not a device node"));
    };
    let rdev = md.rdev();
    Ok(DevRule { ty, major: Some(libc::major(rdev)), minor: Some(libc::minor(rdev)), access })
}

/// "char-NAME"/"block-NAME": every major whose /proc/devices name matches the
/// glob NAME; "*" is the whole type.
pub fn class_rules(ty: u32, name: &str, access: u32, proc_devices: &str) -> Vec<DevRule> {
    if name == "*" {
        return vec![DevRule { ty, major: None, minor: None, access }];
    }
    let header = if ty == DEV_CHAR { "Character devices:" } else { "Block devices:" };
    let Ok(pattern) = CString::new(name) else { return Vec::new() };
    let mut out: Vec<DevRule> = Vec::new();
    let mut in_section = false;
    for line in proc_devices.lines() {
        let line = line.trim();
        if line.ends_with(':') {
            in_section = line == header;
            continue;
        }
        let Some((major, dev)) = line.split_once(char::is_whitespace) else { continue };
        let (Ok(major), Ok(dev)) = (major.parse::<u32>(), CString::new(dev.trim())) else { continue };
        if in_section && unsafe { libc::fnmatch(pattern.as_ptr(), dev.as_ptr(), 0) } == 0 {
            let r = DevRule { ty, major: Some(major), minor: None, access };
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// The allow list for DevicePolicy=/DeviceAllow= (everything else is denied),
/// or None when no program is needed: policy auto without DeviceAllow=.
/// PrivateDevices= turns auto into closed, as in systemd.
pub fn device_rules(unit: &Unit, sec: &str, msgs: &mut Vec<String>) -> Option<Vec<DevRule>> {
    let allow = unit.get_all(sec, "DeviceAllow");
    let mut policy = unit.get(sec, "DevicePolicy").unwrap_or_else(|| "auto".into());
    if policy == "auto" && sec == "Service" && unit.get_bool("Service", "PrivateDevices", false) {
        policy = "closed".into();
    }
    match policy.as_str() {
        "auto" if allow.is_empty() => return None,
        "auto" | "closed" | "strict" => {}
        other => {
            msgs.push(format!("DevicePolicy={other} unknown, ignored"));
            return None;
        }
    }
    let proc_devices = fs::read_to_string("/proc/devices").unwrap_or_default();
    let mut rules = Vec::new();
    if policy != "strict" {
        rules.extend(AUTO_DEVICES.iter().filter_map(|p| node_rule(p, ACC_ALL).ok()));
        rules.extend(class_rules(DEV_CHAR, "pts", ACC_READ | ACC_WRITE, &proc_devices));
    }
    for entry in allow {
        let mut words = entry.split_whitespace();
        let Some(spec) = words.next() else { continue };
        let access = match words.next() {
            None => ACC_ALL,
            Some(a) => match parse_access(a) {
                Some(acc) => acc,
                None => {
                    msgs.push(format!("DeviceAllow={entry}: bad access \"{a}\", ignored"));
                    continue;
                }
            },
        };
        if spec.starts_with("/dev/") || spec.starts_with("/run/systemd/inaccessible/") {
            match node_rule(spec, access) {
                Ok(r) => rules.push(r),
                // Units list devices that may not exist here (/dev/kvm).
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => msgs.push(format!("DeviceAllow={spec}: {e}, ignored")),
            }
        } else if let Some(name) = spec.strip_prefix("char-") {
            rules.extend(class_rules(DEV_CHAR, name, access, &proc_devices));
        } else if let Some(name) = spec.strip_prefix("block-") {
            rules.extend(class_rules(DEV_BLOCK, name, access, &proc_devices));
        } else {
            msgs.push(format!("DeviceAllow={spec}: not a /dev path or char-/block- class, ignored"));
        }
    }
    Some(rules)
}

/// Allow what matches a rule, deny the rest. The context is struct
/// bpf_cgroup_dev_ctx { u32 access_type (access << 16 | type); u32 major; u32 minor; }.
pub fn device_program(rules: &[DevRule]) -> Vec<Insn> {
    let mut a = Asm::default();
    a.op(LDX_MEM_W, 2, 1, 0, 0);
    a.op(ALU_AND_K, 2, 0, 0, 0xffff); // r2 = type
    a.op(LDX_MEM_W, 3, 1, 0, 0);
    a.op(ALU_RSH_K, 3, 0, 0, 16); // r3 = requested access
    a.op(LDX_MEM_W, 4, 1, 4, 0); // r4 = major
    a.op(LDX_MEM_W, 5, 1, 8, 0); // r5 = minor
    for r in rules {
        let next = a.label();
        if r.access != ACC_ALL {
            // Requested access must be a subset of the allowed one.
            a.op(ALU_MOV_X, 1, 3, 0, 0);
            a.op(ALU_AND_K, 1, 0, 0, (!r.access & ACC_ALL) as i32);
            a.jump(JMP_JNE_K, 1, 0, next);
        }
        a.jump(JMP_JNE_K, 2, r.ty as i32, next);
        if let Some(m) = r.major {
            a.jump(JMP_JNE_K, 4, m as i32, next);
        }
        if let Some(m) = r.minor {
            a.jump(JMP_JNE_K, 5, m as i32, next);
        }
        a.ret(1);
        a.bind(next);
    }
    a.ret(0);
    a.finish()
}

// --- IPAddressAllow= / IPAddressDeny= ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    V4([u8; 4], u8),
    V6([u8; 16], u8),
}

fn mask_bytes<const N: usize>(len: u8) -> [u8; N] {
    let mut m = [0u8; N];
    for (i, b) in m.iter_mut().enumerate() {
        let bits = (len as usize).saturating_sub(i * 8).min(8);
        *b = !(0xffu16 >> bits) as u8;
    }
    m
}

impl Prefix {
    /// Host bits cleared.
    fn new_v4(addr: Ipv4Addr, len: u8) -> Prefix {
        let m = mask_bytes::<4>(len);
        let mut a = addr.octets();
        a.iter_mut().zip(m).for_each(|(b, m)| *b &= m);
        Prefix::V4(a, len)
    }

    fn new_v6(addr: Ipv6Addr, len: u8) -> Prefix {
        let m = mask_bytes::<16>(len);
        let mut a = addr.octets();
        a.iter_mut().zip(m).for_each(|(b, m)| *b &= m);
        Prefix::V6(a, len)
    }
}

/// Address list words: an address with an optional /prefix length, or one of
/// systemd's names (any, localhost, link-local, multicast).
pub fn parse_prefixes(words: &[String], key: &str, msgs: &mut Vec<String>) -> Vec<Prefix> {
    let mut out = Vec::new();
    for w in words {
        let named: &[(&str, u8)] = match w.as_str() {
            "any" => &[("0.0.0.0", 0), ("::", 0)],
            "localhost" => &[("127.0.0.0", 8), ("::1", 128)],
            "link-local" => &[("169.254.0.0", 16), ("fe80::", 64)],
            "multicast" => &[("224.0.0.0", 4), ("ff00::", 8)],
            _ => &[],
        };
        if !named.is_empty() {
            for (a, len) in named {
                out.push(match a.parse::<Ipv4Addr>() {
                    Ok(v4) => Prefix::new_v4(v4, *len),
                    Err(_) => Prefix::new_v6(a.parse().expect("built-in address"), *len),
                });
            }
            continue;
        }
        let (addr, len) = match w.split_once('/') {
            Some((a, l)) => (a, l.parse::<u8>().ok()),
            None => (w.as_str(), None),
        };
        let p = if let Ok(v4) = addr.parse::<Ipv4Addr>() {
            len.unwrap_or(32).le(&32).then(|| Prefix::new_v4(v4, len.unwrap_or(32)))
        } else if let Ok(v6) = addr.parse::<Ipv6Addr>() {
            len.unwrap_or(128).le(&128).then(|| Prefix::new_v6(v6, len.unwrap_or(128)))
        } else {
            None
        };
        match p {
            Some(p) if w.contains('/') == len.is_some() => {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            _ => msgs.push(format!("{key}={w}: not an address prefix, ignored")),
        }
    }
    out
}

/// systemd's rule: a packet whose address (the source on ingress, the
/// destination on egress) is on the allow list passes; otherwise one on the
/// deny list is dropped; everything else passes. The context is __sk_buff;
/// skb->data points at the network header.
pub fn firewall_program(allow: &[Prefix], deny: &[Prefix], ingress: bool) -> Vec<Insn> {
    let mut a = Asm::default();
    let accept = a.label();
    let drop = a.label();
    let has_deny_v4 = deny.iter().any(|p| matches!(p, Prefix::V4(..)));
    let has_deny_v6 = deny.iter().any(|p| matches!(p, Prefix::V6(..)));
    let v4 = a.label();
    let v6 = a.label();
    a.op(ALU64_MOV_X, 6, 1, 0, 0); // r6 = skb
    a.op(LDX_MEM_W, 2, 6, 16, 0); // r2 = skb->protocol (__be16)
    if has_deny_v4 {
        a.jump(JMP_JEQ_K, 2, 0x0800u16.to_be() as i32, v4);
    }
    if has_deny_v6 {
        a.jump(JMP_JEQ_K, 2, 0x86ddu16.to_be() as i32, v6);
    }
    a.jump(JMP_JA, 0, 0, accept);

    // Address bytes go to the stack at r10-16 (bpf_skb_load_bytes), then are
    // compared a 32-bit word at a time, masked, in network byte order.
    let family = |a: &mut Asm, label: usize, offset: i32, len: i32, want_v4: bool| {
        a.bind(label);
        a.op(ALU64_MOV_X, 1, 6, 0, 0);
        a.op(ALU64_MOV_K, 2, 0, 0, offset);
        a.op(ALU64_MOV_X, 3, 10, 0, 0);
        a.op(ALU64_ADD_K, 3, 0, 0, -16);
        a.op(ALU64_MOV_K, 4, 0, 0, len);
        a.op(JMP_CALL, 0, 0, 0, FUNC_SKB_LOAD_BYTES);
        a.jump(JMP_JNE_K, 0, 0, accept); // short packet
        // A /0 prefix matches every address, so it ends the family: the
        // verifier refuses unreachable instructions after it.
        for (list, target) in [(allow, accept), (deny, drop)] {
            for p in list {
                let (addr, plen): (&[u8], u8) = match p {
                    Prefix::V4(b, l) if want_v4 => (b, *l),
                    Prefix::V6(b, l) if !want_v4 => (b, *l),
                    _ => continue,
                };
                if plen == 0 {
                    a.jump(JMP_JA, 0, 0, target);
                    return;
                }
                let mask: Vec<u8> = if want_v4 { mask_bytes::<4>(plen).to_vec() } else { mask_bytes::<16>(plen).to_vec() };
                let next = a.label();
                for w in 0..addr.len() / 4 {
                    let m = u32::from_ne_bytes(mask[w * 4..w * 4 + 4].try_into().unwrap());
                    if m == 0 {
                        continue;
                    }
                    let v = u32::from_ne_bytes(addr[w * 4..w * 4 + 4].try_into().unwrap());
                    a.op(LDX_MEM_W, 1, 10, -16 + 4 * w as i16, 0);
                    if m != u32::MAX {
                        a.op(ALU_AND_K, 1, 0, 0, m as i32);
                    }
                    a.jump(JMP32_JNE_K, 1, v as i32, next);
                }
                a.jump(JMP_JA, 0, 0, target);
                a.bind(next);
            }
        }
        a.jump(JMP_JA, 0, 0, accept);
    };
    // IPv4: saddr at 12, daddr at 16. IPv6: saddr at 8, daddr at 24.
    if has_deny_v4 {
        family(&mut a, v4, if ingress { 12 } else { 16 }, 4, true);
    } else {
        a.bind(v4);
    }
    if has_deny_v6 {
        family(&mut a, v6, if ingress { 8 } else { 24 }, 16, false);
    } else {
        a.bind(v6);
    }
    // Nothing jumps to it when an allow-all prefix shadows the deny list.
    if a.fixups.iter().any(|&(_, l)| l == drop) {
        a.bind(drop);
        a.ret(0);
    }
    a.bind(accept);
    a.ret(1);
    a.finish()
}

// --- Kernel interface ---

const BPF_PROG_LOAD: i32 = 5;
const BPF_PROG_ATTACH: i32 = 8;
const BPF_PROG_DETACH: i32 = 9;
const BPF_PROG_GET_FD_BY_ID: i32 = 13;
const BPF_OBJ_GET_INFO_BY_FD: i32 = 15;
const BPF_PROG_QUERY: i32 = 16;
pub const PROG_TYPE_CGROUP_SKB: u32 = 8;
pub const PROG_TYPE_CGROUP_DEVICE: u32 = 15;
pub const ATTACH_INET_INGRESS: u32 = 0;
pub const ATTACH_INET_EGRESS: u32 = 1;
pub const ATTACH_DEVICE: u32 = 6;
const F_ALLOW_MULTI: u32 = 2;

/// union bpf_attr, filled by field offset.
#[repr(C, align(8))]
struct Attr([u8; 128]);

impl Attr {
    fn new() -> Attr {
        Attr([0; 128])
    }

    fn set_u32(&mut self, off: usize, v: u32) {
        self.0[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }

    fn set_u64(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_ne_bytes());
    }

    fn get_u32(&self, off: usize) -> u32 {
        u32::from_ne_bytes(self.0[off..off + 4].try_into().unwrap())
    }
}

fn sys_bpf(cmd: i32, attr: &mut Attr) -> io::Result<i32> {
    let r = unsafe { libc::syscall(libc::SYS_bpf, cmd, attr.0.as_mut_ptr(), attr.0.len()) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r as i32)
    }
}

/// BPF_PROG_LOAD. On failure, the verifier's last words go into the error.
pub fn load(prog_type: u32, attach_type: u32, name: &str, insns: &[Insn]) -> Result<OwnedFd, String> {
    let code: Vec<u8> = insns.iter().flat_map(|i| i.encode()).collect();
    let attempt = |log: &mut [u8]| {
        let mut a = Attr::new();
        a.set_u32(0, prog_type);
        a.set_u32(4, insns.len() as u32);
        a.set_u64(8, code.as_ptr() as u64);
        a.set_u64(16, c"GPL".as_ptr() as u64);
        if !log.is_empty() {
            a.set_u32(24, 1);
            a.set_u32(28, log.len() as u32);
            a.set_u64(32, log.as_mut_ptr() as u64);
        }
        let n = name.len().min(15);
        a.0[48..48 + n].copy_from_slice(&name.as_bytes()[..n]);
        a.set_u32(68, attach_type);
        sys_bpf(BPF_PROG_LOAD, &mut a)
    };
    let fd = match attempt(&mut []) {
        Ok(fd) => fd,
        Err(e) => {
            let mut log = vec![0u8; 1 << 16];
            let _ = attempt(&mut log);
            let text = String::from_utf8_lossy(&log[..log.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(3)..].join(" | ");
            return Err(if tail.is_empty() { format!("load {name}: {e}") } else { format!("load {name}: {e}: {tail}") });
        }
    };
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn prog_name(prog: &OwnedFd) -> Option<String> {
    // struct bpf_prog_info: name[16] at offset 64.
    let mut info = [0u64; 32];
    let mut a = Attr::new();
    a.set_u32(0, prog.as_raw_fd() as u32);
    a.set_u32(4, std::mem::size_of_val(&info) as u32);
    a.set_u64(8, info.as_mut_ptr() as u64);
    sys_bpf(BPF_OBJ_GET_INFO_BY_FD, &mut a).ok()?;
    let bytes: Vec<u8> = info.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let name = &bytes[64..80];
    Some(String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(16)]).into_owned())
}

/// Detach the programs called `name` attached directly to the cgroup.
pub fn detach_named(cgroup: &OwnedFd, attach_type: u32, name: &str) -> io::Result<()> {
    let mut ids = [0u32; 64];
    let mut a = Attr::new();
    a.set_u32(0, cgroup.as_raw_fd() as u32);
    a.set_u32(4, attach_type);
    a.set_u64(16, ids.as_mut_ptr() as u64);
    a.set_u32(24, ids.len() as u32);
    sys_bpf(BPF_PROG_QUERY, &mut a)?;
    let count = (a.get_u32(24) as usize).min(ids.len());
    for &id in &ids[..count] {
        let mut g = Attr::new();
        g.set_u32(0, id);
        let Ok(fd) = sys_bpf(BPF_PROG_GET_FD_BY_ID, &mut g) else { continue };
        let prog = unsafe { OwnedFd::from_raw_fd(fd) };
        if prog_name(&prog).as_deref() == Some(name) {
            let mut d = Attr::new();
            d.set_u32(0, cgroup.as_raw_fd() as u32);
            d.set_u32(4, prog.as_raw_fd() as u32);
            d.set_u32(8, attach_type);
            sys_bpf(BPF_PROG_DETACH, &mut d)?;
        }
    }
    Ok(())
}

pub fn attach(cgroup: &OwnedFd, prog: &OwnedFd, attach_type: u32) -> io::Result<()> {
    let mut a = Attr::new();
    a.set_u32(0, cgroup.as_raw_fd() as u32);
    a.set_u32(4, prog.as_raw_fd() as u32);
    a.set_u32(8, attach_type);
    a.set_u32(12, F_ALLOW_MULTI);
    sys_bpf(BPF_PROG_ATTACH, &mut a).map(|_| ())
}

pub fn open_cgroup(dir: &Path) -> io::Result<OwnedFd> {
    let c = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Replace the unit's programs on its cgroup with the ones its settings ask
/// for (none: just detach the previous start's). Returns messages to log;
/// like systemd, a program that cannot be attached does not fail the start.
pub fn apply(unit: &Unit, cgroup: &Path) -> Vec<String> {
    let sec = unit.type_section();
    let mut msgs = Vec::new();
    let devices = device_rules(unit, &sec, &mut msgs).map(|r| device_program(&r));
    let allow = parse_prefixes(&unit.get_words(&sec, "IPAddressAllow"), "IPAddressAllow", &mut msgs);
    let deny = parse_prefixes(&unit.get_words(&sec, "IPAddressDeny"), "IPAddressDeny", &mut msgs);
    // Without a deny list nothing is dropped.
    let firewall = !deny.is_empty();
    let progs = [
        ("DevicePolicy=", PROG_TYPE_CGROUP_DEVICE, ATTACH_DEVICE, "sd_devices", devices),
        ("IPAddressDeny=", PROG_TYPE_CGROUP_SKB, ATTACH_INET_INGRESS, "sd_fw_ingress", firewall.then(|| firewall_program(&allow, &deny, true))),
        ("IPAddressDeny=", PROG_TYPE_CGROUP_SKB, ATTACH_INET_EGRESS, "sd_fw_egress", firewall.then(|| firewall_program(&allow, &deny, false))),
    ];
    let cg = match open_cgroup(cgroup) {
        Ok(fd) => fd,
        Err(e) => {
            if progs.iter().any(|p| p.4.is_some()) {
                msgs.push(format!("{}: {e}", cgroup.display()));
            }
            return msgs;
        }
    };
    for (setting, prog_type, attach_type, name, insns) in progs {
        let res = detach_named(&cg, attach_type, name);
        let Some(insns) = insns else { continue };
        let res = res
            .map_err(|e| format!("query {name}: {e}"))
            .and_then(|_| load(prog_type, attach_type, name, &insns))
            .and_then(|prog| attach(&cg, &prog, attach_type).map_err(|e| format!("attach {name}: {e}")));
        if let Err(e) = res {
            msgs.push(format!("{setting} not enforced: {e}"));
        }
    }
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    /// Runs the subset of eBPF these programs use. Pointers are tagged: the
    /// context lives at CTX, the stack below STACK.
    fn run(insns: &[Insn], ctx: &[u8], packet: &[u8]) -> u64 {
        const CTX: u64 = 0x1000_0000;
        const STACK: u64 = 0x2000_0000;
        let mut stack = [0u8; 512];
        let mut r = [0u64; 11];
        r[1] = CTX;
        r[10] = STACK;
        let mut pc = 0usize;
        let read32 = |stack: &[u8; 512], addr: u64| -> u32 {
            let b: [u8; 4] = if (CTX..CTX + ctx.len() as u64).contains(&addr) {
                let o = (addr - CTX) as usize;
                ctx[o..o + 4].try_into().unwrap()
            } else {
                let o = 512 - (STACK - addr) as usize;
                stack[o..o + 4].try_into().unwrap()
            };
            u32::from_ne_bytes(b)
        };
        for _ in 0..10_000 {
            let i = insns[pc];
            let (d, s) = (i.dst as usize, i.src as usize);
            let imm = i.imm as i64 as u64;
            let jump = |cond: bool| if cond { pc + 1 + i.off as usize } else { pc + 1 };
            pc = match i.code {
                LDX_MEM_W => {
                    r[d] = read32(&stack, r[s].wrapping_add(i.off as i64 as u64)) as u64;
                    pc + 1
                }
                ALU64_MOV_X => {
                    r[d] = r[s];
                    pc + 1
                }
                ALU64_MOV_K => {
                    r[d] = imm;
                    pc + 1
                }
                ALU64_ADD_K => {
                    r[d] = r[d].wrapping_add(imm);
                    pc + 1
                }
                ALU_MOV_X => {
                    r[d] = r[s] as u32 as u64;
                    pc + 1
                }
                ALU_AND_K => {
                    r[d] = (r[d] as u32 & i.imm as u32) as u64;
                    pc + 1
                }
                ALU_RSH_K => {
                    r[d] = ((r[d] as u32) >> i.imm) as u64;
                    pc + 1
                }
                JMP_JA => jump(true),
                JMP_JEQ_K => jump(r[d] == imm),
                JMP_JNE_K => jump(r[d] != imm),
                JMP32_JNE_K => jump(r[d] as u32 != i.imm as u32),
                JMP_CALL => {
                    assert_eq!(i.imm, FUNC_SKB_LOAD_BYTES);
                    assert_eq!(r[1], CTX);
                    let (off, len) = (r[2] as usize, r[4] as usize);
                    let o = 512 - (STACK - r[3]) as usize;
                    r[0] = match packet.get(off..off + len) {
                        Some(b) => {
                            stack[o..o + len].copy_from_slice(b);
                            0
                        }
                        None => (-14i64) as u64,
                    };
                    pc + 1
                }
                JMP_EXIT => return r[0],
                c => panic!("opcode {c:#x}"),
            };
        }
        panic!("no exit");
    }

    /// The verifier refuses unreachable instructions and jumps out of range;
    /// the interpreter only follows the paths a test takes.
    fn assert_reachable(insns: &[Insn]) {
        let mut seen = vec![false; insns.len()];
        let mut todo = vec![0usize];
        while let Some(pc) = todo.pop() {
            assert!(pc < insns.len(), "jump out of range");
            if std::mem::replace(&mut seen[pc], true) {
                continue;
            }
            let i = insns[pc];
            let target = (pc as isize + 1 + i.off as isize) as usize;
            match i.code {
                JMP_EXIT => {}
                JMP_JA => todo.push(target),
                JMP_CALL => todo.push(pc + 1),
                c if c & 0x07 == 0x05 || c & 0x07 == 0x06 => todo.extend([pc + 1, target]),
                _ => todo.push(pc + 1),
            }
        }
        let dead: Vec<usize> = (0..insns.len()).filter(|&i| !seen[i]).collect();
        assert!(dead.is_empty(), "unreachable insns {dead:?}");
    }

    #[test]
    fn programs_have_no_dead_code() {
        let mut msgs = Vec::new();
        let lists = ["", "any", "localhost", "192.0.2.0/24 2001:db8::/32", "10.0.0.0/8 ::/0", "0.0.0.0/0"];
        for allow in lists {
            for deny in lists.iter().filter(|d| !d.is_empty()) {
                let allow = parse_prefixes(&words(allow), "IPAddressAllow", &mut msgs);
                let deny = parse_prefixes(&words(deny), "IPAddressDeny", &mut msgs);
                for ingress in [true, false] {
                    assert_reachable(&firewall_program(&allow, &deny, ingress));
                }
            }
        }
        assert!(msgs.is_empty(), "{msgs:?}");
        let rule = DevRule { ty: DEV_CHAR, major: Some(1), minor: None, access: ACC_READ };
        assert_reachable(&device_program(&[]));
        assert_reachable(&device_program(&[rule, DevRule { minor: Some(3), access: ACC_ALL, ..rule }]));
    }

    fn dev_ctx(ty: u32, access: u32, major: u32, minor: u32) -> Vec<u8> {
        [(access << 16) | ty, major, minor].iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn device_program_allows_listed() {
        let rules = [
            DevRule { ty: DEV_CHAR, major: Some(1), minor: Some(3), access: ACC_READ | ACC_WRITE },
            DevRule { ty: DEV_CHAR, major: Some(136), minor: None, access: ACC_ALL },
        ];
        let p = device_program(&rules);
        let check = |ty, acc, maj, min| run(&p, &dev_ctx(ty, acc, maj, min), &[]);
        assert_eq!(check(DEV_CHAR, ACC_READ, 1, 3), 1);
        assert_eq!(check(DEV_CHAR, ACC_READ | ACC_WRITE, 1, 3), 1);
        assert_eq!(check(DEV_CHAR, ACC_MKNOD, 1, 3), 0);
        assert_eq!(check(DEV_CHAR, ACC_READ, 1, 5), 0);
        assert_eq!(check(DEV_BLOCK, ACC_READ, 1, 3), 0);
        assert_eq!(check(DEV_CHAR, ACC_ALL, 136, 7), 1);
        assert_eq!(check(DEV_CHAR, ACC_READ, 137, 7), 0);
        // Empty list (DevicePolicy=strict alone): deny everything.
        assert_eq!(run(&device_program(&[]), &dev_ctx(DEV_CHAR, ACC_READ, 1, 3), &[]), 0);
    }

    fn skb(proto: u16) -> Vec<u8> {
        let mut c = vec![0u8; 64];
        c[16..20].copy_from_slice(&(proto.to_be() as u32).to_ne_bytes());
        c
    }

    fn ipv4(src: [u8; 4], dst: [u8; 4]) -> Vec<u8> {
        let mut p = vec![0u8; 20];
        p[0] = 0x45;
        p[12..16].copy_from_slice(&src);
        p[16..20].copy_from_slice(&dst);
        p
    }

    fn ipv6(src: &str, dst: &str) -> Vec<u8> {
        let mut p = vec![0u8; 40];
        p[0] = 0x60;
        p[8..24].copy_from_slice(&src.parse::<Ipv6Addr>().unwrap().octets());
        p[24..40].copy_from_slice(&dst.parse::<Ipv6Addr>().unwrap().octets());
        p
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn firewall_deny_any_allow_localhost() {
        let mut msgs = Vec::new();
        let allow = parse_prefixes(&words("localhost 10.0.0.0/8 fe80::/64"), "IPAddressAllow", &mut msgs);
        let deny = parse_prefixes(&words("any"), "IPAddressDeny", &mut msgs);
        assert!(msgs.is_empty(), "{msgs:?}");
        let egress = firewall_program(&allow, &deny, false);
        let ingress = firewall_program(&allow, &deny, true);
        let v4 = skb(0x0800);
        let v6 = skb(0x86dd);
        let far = [192, 0, 2, 1];
        let lo = [127, 0, 0, 1];
        assert_eq!(run(&egress, &v4, &ipv4(far, lo)), 1);
        assert_eq!(run(&egress, &v4, &ipv4(lo, far)), 0);
        assert_eq!(run(&egress, &v4, &ipv4(far, [10, 200, 3, 4])), 1);
        assert_eq!(run(&egress, &v4, &ipv4(far, [11, 0, 0, 1])), 0);
        assert_eq!(run(&ingress, &v4, &ipv4(lo, far)), 1);
        assert_eq!(run(&ingress, &v4, &ipv4(far, lo)), 0);
        assert_eq!(run(&egress, &v6, &ipv6("2001:db8::1", "::1")), 1);
        assert_eq!(run(&egress, &v6, &ipv6("::1", "2001:db8::1")), 0);
        assert_eq!(run(&egress, &v6, &ipv6("::1", "fe80::1234:5678")), 1);
        assert_eq!(run(&egress, &v6, &ipv6("::1", "fe80:0:0:1::1")), 0);
        assert_eq!(run(&ingress, &v6, &ipv6("::1", "2001:db8::1")), 1);
        // Not IP (ARP), and a truncated packet: pass.
        assert_eq!(run(&egress, &skb(0x0806), &[]), 1);
        assert_eq!(run(&egress, &v4, &[0x45; 10]), 1);
    }

    #[test]
    fn firewall_deny_one_family() {
        let mut msgs = Vec::new();
        let deny = parse_prefixes(&words("192.0.2.0/24"), "IPAddressDeny", &mut msgs);
        let p = firewall_program(&[], &deny, false);
        assert_eq!(run(&p, &skb(0x0800), &ipv4([1, 1, 1, 1], [192, 0, 2, 77])), 0);
        assert_eq!(run(&p, &skb(0x0800), &ipv4([1, 1, 1, 1], [192, 0, 3, 77])), 1);
        assert_eq!(run(&p, &skb(0x86dd), &ipv6("::1", "2001:db8::1")), 1);
    }

    #[test]
    fn prefixes() {
        let mut msgs = Vec::new();
        let p = parse_prefixes(&words("10.1.2.3/8 ::1 any bogus 1.2.3.4/33 1.2.3.4/"), "IPAddressDeny", &mut msgs);
        assert_eq!(
            p,
            [
                Prefix::V4([10, 0, 0, 0], 8),
                Prefix::V6("::1".parse::<Ipv6Addr>().unwrap().octets(), 128),
                Prefix::V4([0; 4], 0),
                Prefix::V6([0; 16], 0),
            ]
        );
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        assert_eq!(mask_bytes::<4>(12), [0xff, 0xf0, 0, 0]);
    }

    const PROC_DEVICES: &str = "Character devices:\n  1 mem\n  4 /dev/vc/0\n  4 tty\n  4 ttyS\n136 pts\n180 usb\n189 usb_device\n\nBlock devices:\n  7 loop\n259 blkext\n";

    #[test]
    fn device_classes() {
        let r = class_rules(DEV_CHAR, "usb*", ACC_READ, PROC_DEVICES);
        assert_eq!(r.iter().map(|r| r.major.unwrap()).collect::<Vec<_>>(), [180, 189]);
        let r = class_rules(DEV_CHAR, "tty*", ACC_READ, PROC_DEVICES);
        assert_eq!(r.len(), 1);
        assert!(class_rules(DEV_CHAR, "loop", ACC_READ, PROC_DEVICES).is_empty());
        assert_eq!(class_rules(DEV_BLOCK, "loop", ACC_READ, PROC_DEVICES)[0].major, Some(7));
        assert_eq!(class_rules(DEV_BLOCK, "*", ACC_READ, PROC_DEVICES)[0].major, None);
        assert_eq!(parse_access("rw"), Some(ACC_READ | ACC_WRITE));
        assert_eq!(parse_access("rx"), None);
    }

    fn test_unit(text: &str) -> Unit {
        let mut file = crate::unit::UnitFile::default();
        file.parse_str(text);
        Unit {
            id: "t.service".into(),
            names: vec!["t.service".into()],
            fragment: None,
            dropins: Vec::new(),
            load_state: crate::loader::LoadState::Loaded,
            load_error: None,
            file,
        }
    }

    #[test]
    fn device_policies() {
        let mut msgs = Vec::new();
        assert!(device_rules(&test_unit("[Service]\nExecStart=/bin/true\n"), "Service", &mut msgs).is_none());
        // polkit.service: strict, /dev/null only.
        let u = test_unit("[Service]\nDeviceAllow=/dev/null rw\nDevicePolicy=strict\nPrivateDevices=yes\n");
        let r = device_rules(&u, "Service", &mut msgs).unwrap();
        assert_eq!(r, [DevRule { ty: DEV_CHAR, major: Some(1), minor: Some(3), access: ACC_READ | ACC_WRITE }]);
        // PrivateDevices= alone: closed, so the pseudo devices.
        let u = test_unit("[Service]\nPrivateDevices=yes\n");
        let r = device_rules(&u, "Service", &mut msgs).unwrap();
        assert!(r.contains(&DevRule { ty: DEV_CHAR, major: Some(1), minor: Some(9), access: ACC_ALL }));
        // auto with a list: closed plus the list; an empty assignment resets.
        let u = test_unit("[Service]\nDeviceAllow=/dev/zero r\nDeviceAllow=\nDeviceAllow=/dev/does-not-exist rw\n");
        let r = device_rules(&u, "Service", &mut msgs).unwrap();
        assert!(!r.contains(&DevRule { ty: DEV_CHAR, major: Some(1), minor: Some(5), access: ACC_READ }));
        assert!(r.contains(&DevRule { ty: DEV_CHAR, major: Some(1), minor: Some(5), access: ACC_ALL }));
        assert!(msgs.is_empty(), "{msgs:?}");
        let u = test_unit("[Service]\nDeviceAllow=/dev/null rx\nDeviceAllow=sda rw\n");
        device_rules(&u, "Service", &mut msgs);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
    }

    /// Loads and attaches the programs to a scratch cgroup and checks them
    /// from a child inside it. Needs root and cgroup2:
    ///   tests/bpf-root.sh
    #[test]
    #[ignore]
    fn kernel_enforces() {
        let dir = Path::new(crate::cgroup::CGROUP_ROOT).join(format!("sdcompat-bpf-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        // Removed even when an assert fails (a cgroup with programs attached
        // can be removed once empty; the child is reaped before this runs).
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir(&self.0);
            }
        }
        let _scratch = Scratch(dir.clone());
        let unit = test_unit("[Service]\nDevicePolicy=strict\nDeviceAllow=/dev/null rw\nIPAddressDeny=any\nIPAddressAllow=localhost\n");
        // Twice: the second start must replace, not add to, the first's.
        assert_eq!(apply(&unit, &dir), Vec::<String>::new());
        assert_eq!(apply(&unit, &dir), Vec::<String>::new());
        let cg = open_cgroup(&dir).unwrap();
        for t in [ATTACH_DEVICE, ATTACH_INET_INGRESS, ATTACH_INET_EGRESS] {
            let mut ids = [0u32; 8];
            let mut a = Attr::new();
            a.set_u32(0, cg.as_raw_fd() as u32);
            a.set_u32(4, t);
            a.set_u64(16, ids.as_mut_ptr() as u64);
            a.set_u32(24, ids.len() as u32);
            sys_bpf(BPF_PROG_QUERY, &mut a).unwrap();
            assert_eq!(a.get_u32(24), 1, "attach type {t}");
        }

        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let _ = fs::write(dir.join("cgroup.procs"), "0");
            // 0 on success, else the errno.
            let errno = || io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            let open = |p: &CStr| if unsafe { libc::open(p.as_ptr(), libc::O_RDONLY) } >= 0 { 0 } else { errno() };
            let send = |addr: [u8; 4]| unsafe {
                let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
                let mut sa: libc::sockaddr_in = std::mem::zeroed();
                sa.sin_family = libc::AF_INET as libc::sa_family_t;
                sa.sin_port = 9u16.to_be();
                sa.sin_addr.s_addr = u32::from_ne_bytes(addr);
                let sp = &sa as *const libc::sockaddr_in as *const libc::sockaddr;
                let r = libc::sendto(s, b"x".as_ptr() as *const libc::c_void, 1, 0, sp, std::mem::size_of_val(&sa) as u32);
                if r == 1 { 0 } else { errno() }
            };
            let bits = [
                open(c"/dev/null") == 0,
                open(c"/dev/zero") == libc::EPERM,
                send([127, 0, 0, 1]) == 0,
                send([192, 0, 2, 1]) == libc::EPERM,
            ];
            let code = bits.iter().enumerate().fold(0, |c, (i, ok)| if *ok { c } else { c | 1 << i });
            unsafe { libc::_exit(code) };
        }
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0, "failed checks (bits: null, zero, localhost, far)");
    }

}
