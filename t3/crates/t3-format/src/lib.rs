//! `t3-format` — zero-copy, panic-free container triage.
//!
//! Turns a byte buffer into [`t3_core::Facts`]: a fixed table of tri-state
//! observations. Two properties are held above all others:
//!
//! 1. **Malformed input is an answer, not an error.** A truncated file parses and
//!    reports which facts became unmeasurable. Nothing panics; nothing silently
//!    defaults to `false`.
//! 2. **No allocation on the decision path.** The section table is a fixed array;
//!    the fact table is a fixed array; import names are borrowed from the buffer.
//!    Only the region list allocates, because its length depends on the file.
//!
//! Scope is metadata only: headers, tables, names, entropy. There is no
//! disassembly here, and consequently no claim about what the code *does* — the
//! facts describe an attack surface, not behaviour.

#![deny(missing_docs)]
#![warn(clippy::pedantic)]

pub mod detect;
pub mod elf;
pub mod pe;
pub mod reader;
pub mod region;

pub use detect::{analyze, calibration, presence, Analysis, Kind};
pub use elf::{ElfError, ElfInfo};
pub use pe::{PeError, PeInfo};
pub use reader::{Endian, Reader};
pub use region::{Region, RegionKind};
