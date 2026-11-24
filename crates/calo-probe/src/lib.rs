//! Calorimeter measurement probe (`calo-probe`, Spec §9).
//!
//! `unsafe` is strictly confined to `crate::sys` and (in P10) `crate::gpu::ffi`
//! per invariant I8. All other modules carry `#![forbid(unsafe_code)]`.

pub mod channel;
pub mod clock;
pub mod counters;
pub mod discover;
pub mod gpu;
pub mod observe;
pub mod probe;
pub mod profile;
pub mod scalars;
pub mod series;
pub mod spawn;
pub mod sys;
pub mod table_writer;
