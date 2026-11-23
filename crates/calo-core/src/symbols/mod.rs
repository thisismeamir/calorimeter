//! Symbolization trait, C++/Rust demangling, template folding, and classification (spec §12.1 steps 3–4).

pub mod error;
pub mod normalizer;

pub use error::SymbolError;
pub use normalizer::{
    demangle_symbol, fold_templates, FakeSymbolizer, Frame, Normalizer, RawFrame, Symbolizer,
};
