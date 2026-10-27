//! What every uinitconverter crate shares: unit file parsing and loading,
//! uinitconverter.conf and the programs it names, the per-unit state files, the generated database's index,
//! and the fd store protocol.

pub mod automask;
pub mod calendar;
pub mod config;
pub mod envfile;
pub mod fdstore;
pub mod index;
pub mod loader;
pub mod mountunit;
pub mod pathspec;
pub mod state;
pub mod timerspec;
pub mod tools;
pub mod unit;
pub mod util;
