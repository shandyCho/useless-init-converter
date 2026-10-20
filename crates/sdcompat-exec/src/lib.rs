//! How sdcompat sets up a service's process, as systemd's exec context does:
//! sandboxing and namespaces, seccomp, cgroup BPF, cgroups, SELinux labels,
//! PAM sessions, listening sockets, command lines and conditions.

pub use sdcompat_base::{config, envfile, loader, state, unit, util};

pub mod bpf;
pub mod cgroup;
pub mod conditions;
pub mod execcmd;
pub mod pam;
pub mod sandbox;
pub mod seccomp;
pub mod selinux;
pub mod socket;
