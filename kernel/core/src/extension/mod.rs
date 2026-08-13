// SPDX-License-Identifier: MPL-2.0

//! A minimal, interpreted kernel-extension prototype.
//!
//! This module deliberately has no user-facing ABI yet. It establishes the
//! trusted path from fixed-width program bytes, through verification, to a
//! read-only system-call hook and a small set of safe helpers.

mod interpreter;
mod isa;
mod program;
pub(super) mod syscall_observer;

#[cfg(ktest)]
mod tests;
