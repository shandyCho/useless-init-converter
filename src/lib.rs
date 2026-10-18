//! sdcompat: run systemd units on s6-rc and serve systemd's D-Bus API.

pub mod calendar;
pub mod cgroup;
pub mod client;
pub mod conditions;
pub mod config;
pub mod daemon;
pub mod envfile;
pub mod execcmd;
pub mod gen;
pub mod install;
pub mod loader;
pub mod mountunit;
pub mod pam;
pub mod pathunit;
pub mod runner;
pub mod sandbox;
pub mod socket;
pub mod state;
pub mod timer;
pub mod unit;
pub mod util;
