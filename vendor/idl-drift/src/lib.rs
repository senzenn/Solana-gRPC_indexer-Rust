//! `idl-drift` — semantic drift detection for Solana program IDLs.
//!
//! Diffs two IDL versions and classifies every change as
//! Breaking / Dangerous / Additive / Cosmetic, oriented around whether a
//! Vixen-generated parser would misread data after the change.
//!
//! The classification rules are grounded in how Vixen's generated parsers read
//! bytes (positional accounts, `check_min_accounts_req`, discriminator
//! dispatch, tag-indexed `oneof` enums) and validated against an awk oracle
//! (`proto/diff.awk`) covering 16 fixtures.
#![warn(clippy::pedantic)]
// Match Vixen's own crate posture (see crates/parser/src/lib.rs): pedantic
// warnings on, but docs/errors-doc/panics-doc allowances so the gate is
// identical to the workspace it will live in.
#![allow(
    missing_docs,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions
)]

pub mod codama_convert;
pub mod diff;
pub mod inspect;
pub mod model;

pub use diff::{diff, Change, Report, Severity};
pub use inspect::{coverage, CoverageReport, UnmappedField};
