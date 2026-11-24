//! Symbolizer interface, demangling, `fold_templates`, and `Normalizer` (spec §12.1 steps 2–4).

use crate::symbols::error::SymbolError;
use calo_schema::suite::{SymbolClassRule, SymbolFoldRule};
use cpp_demangle::DemangleOptions;
use regex::Regex;
use std::collections::BTreeMap;

/// Raw resolved symbol frame returned by a `Symbolizer` implementation (spec §12.1 step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    /// Mangled or demangled symbol name.
    pub symbol: String,
    /// Source file path if resolved.
    pub file: Option<String>,
    /// Source line number if resolved.
    pub line: Option<u32>,
}

/// Normalized and classified call-stack frame produced by `Normalizer` (spec §12.1 steps 3–4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Demangled and folded symbol name.
    pub name: String,
    /// Assigned classification label (e.g. `"mpi"`, `"omp"`, `"fft"`, `"memory"`, `"io"`).
    pub class: Option<String>,
    /// Source file path if available.
    pub file: Option<String>,
    /// Source line number if available.
    pub line: Option<u32>,
}

/// Pluggable address-to-symbol resolver implemented by the control-plane symbolizer (spec §12.1 step 2).
pub trait Symbolizer: Send + Sync {
    /// Resolves `(build_id, addr)` to a `RawFrame` if symbol information is available.
    fn resolve(&self, build_id: &str, addr: u64) -> Option<RawFrame>;
}

/// Table-driven `Symbolizer` for unit and integration tests (spec §12.1).
#[derive(Debug, Clone, Default)]
pub struct FakeSymbolizer {
    entries: BTreeMap<(String, u64), RawFrame>,
}

impl FakeSymbolizer {
    /// Creates an empty `FakeSymbolizer`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a mapping from `(build_id, addr)` to `frame`.
    pub fn insert(&mut self, build_id: impl Into<String>, addr: u64, frame: RawFrame) {
        self.entries.insert((build_id.into(), addr), frame);
    }
}

impl Symbolizer for FakeSymbolizer {
    fn resolve(&self, build_id: &str, addr: u64) -> Option<RawFrame> {
        self.entries.get(&(build_id.to_string(), addr)).cloned()
    }
}

/// Demangles a Rust (v0/legacy) or C++ Itanium ABI symbol name, falling back to `raw` if demangling fails (spec §12.1 step 3).
pub fn demangle_symbol(raw: &str) -> String {
    let looks_like_rust = raw.starts_with("_R")
        || (raw.starts_with("_ZN")
            && raw.ends_with('E')
            && raw.len() > 20
            && raw[..raw.len() - 17].ends_with("17h")
            && raw[raw.len() - 17..raw.len() - 1]
                .chars()
                .all(|c| c.is_ascii_hexdigit()));

    if looks_like_rust {
        if let Ok(rust_demangled) = rustc_demangle::try_demangle(raw) {
            return format!("{rust_demangled:#}");
        }
    }

    if raw.starts_with("_Z") || raw.starts_with("___Z") {
        if let Ok(sym) = cpp_demangle::Symbol::new(raw) {
            let options = DemangleOptions::default();
            if let Ok(demangled) = sym.demangle(&options) {
                return demangled;
            }
        }
    }

    raw.to_string()
}

/// Collapses nested template argument lists `<...>` beyond `max_depth` into `<...>` (spec §12.1 step 3).
///
/// Handles unbalanced `<` / `>` and C++ `operator<` / `operator<<` / `operator->` tokens safely without panicking.
pub fn fold_templates(name: &str, max_depth: usize) -> String {
    if !name.contains('<') {
        return name.to_string();
    }

    let bytes = name.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut depth: usize = 0;
    let mut i = 0;

    while i < len {
        // Preserve `operator<<`, `operator<`, `operator>>`, `operator>`, `operator->`, `->`
        if bytes[i] == b'-' && i + 1 < len && bytes[i + 1] == b'>' {
            if depth <= max_depth {
                out.push_str("->");
            }
            i += 2;
            continue;
        }
        if name[i..].starts_with("operator<<") || name[i..].starts_with("operator>>") {
            if depth <= max_depth {
                out.push_str(&name[i..i + 10]);
            }
            i += 10;
            continue;
        }
        if name[i..].starts_with("operator<=") || name[i..].starts_with("operator>=") {
            if depth <= max_depth {
                out.push_str(&name[i..i + 10]);
            }
            i += 10;
            continue;
        }
        if name[i..].starts_with("operator<") || name[i..].starts_with("operator>") {
            if depth <= max_depth {
                out.push_str(&name[i..i + 9]);
            }
            i += 9;
            continue;
        }

        let ch = name[i..].chars().next().unwrap_or('\0');
        let ch_len = ch.len_utf8();

        match ch {
            '<' => {
                depth += 1;
                if depth <= max_depth {
                    out.push('<');
                } else if depth == max_depth + 1 {
                    out.push_str("<...>");
                }
            }
            '>' => {
                if depth == 0 {
                    // Unbalanced `>` at top level: preserve literally
                    out.push('>');
                } else {
                    if depth <= max_depth {
                        out.push('>');
                    }
                    depth -= 1;
                }
            }
            _ => {
                if depth <= max_depth {
                    out.push(ch);
                }
            }
        }
        i += ch_len;
    }

    // If brackets were unbalanced (unclosed `<`), return the original string safely
    if depth != 0 {
        return name.to_string();
    }

    out
}

struct CompiledFoldRule {
    regex: Regex,
    replace_as: String,
}

struct CompiledClassRule {
    regex: Regex,
    class: String,
}

struct BuiltinClassRule {
    regex: Regex,
    class: &'static str,
}

/// Compiled symbol normalization and classification engine (spec §12.1 steps 3–4).
pub struct Normalizer {
    fold_rules: Vec<CompiledFoldRule>,
    class_rules: Vec<CompiledClassRule>,
    builtin_classes: Vec<BuiltinClassRule>,
    template_max_depth: Option<usize>,
}

impl Normalizer {
    /// Compiles user `fold` and `classes` rules plus built-in classification rules (spec §12.1).
    pub fn new(fold: &[SymbolFoldRule], classes: &[SymbolClassRule]) -> Result<Self, SymbolError> {
        let mut fold_rules = Vec::with_capacity(fold.len());
        for (idx, rule) in fold.iter().enumerate() {
            let regex = Regex::new(&rule.match_regex).map_err(|e| SymbolError::InvalidRegex {
                kind: "fold",
                index: idx,
                pattern: rule.match_regex.clone(),
                message: e.to_string(),
            })?;
            fold_rules.push(CompiledFoldRule {
                regex,
                replace_as: rule.replace_as.clone(),
            });
        }

        let mut class_rules = Vec::with_capacity(classes.len());
        for (idx, rule) in classes.iter().enumerate() {
            let regex = Regex::new(&rule.match_regex).map_err(|e| SymbolError::InvalidRegex {
                kind: "classes",
                index: idx,
                pattern: rule.match_regex.clone(),
                message: e.to_string(),
            })?;
            class_rules.push(CompiledClassRule {
                regex,
                class: rule.class.clone(),
            });
        }

        let builtin_specs: &[(&str, &str)] = &[
            (r"^(P?MPI_)|(^ompi_)|PMPI", "mpi"),
            (r"(^GOMP_)|(^__kmp)|(^omp_)", "omp"),
            (
                r"\b(memcpy|memset|malloc|free|calloc|realloc)\b|operator (new|delete)",
                "memory",
            ),
            (r"\b(read|write|fwrite|fread)\b|H5|hdf5|adios", "io"),
            (r"(cudaMemcpy|hipMemcpy)", "memcpy"),
            (r"Synchronize", "sync"),
            (r"^(cudaLaunch|hipLaunch|__global__ )", "compute"),
        ];

        let mut builtin_classes = Vec::with_capacity(builtin_specs.len());
        for &(pat, cls) in builtin_specs {
            if let Ok(regex) = Regex::new(pat) {
                builtin_classes.push(BuiltinClassRule { regex, class: cls });
            }
        }

        Ok(Self {
            fold_rules,
            class_rules,
            builtin_classes,
            template_max_depth: None,
        })
    }

    /// Enables built-in template depth collapsing (`fold: {templates: max_depth}`, spec §12.1).
    pub fn with_fold_templates(mut self, max_depth: usize) -> Self {
        self.template_max_depth = Some(max_depth);
        self
    }

    /// Demangles, folds, and classifies a `RawFrame` into a normalized `Frame` (spec §12.1 steps 3–4).
    pub fn normalize(&self, raw: RawFrame) -> Frame {
        let mut name = demangle_symbol(&raw.symbol);

        if let Some(max_depth) = self.template_max_depth {
            name = fold_templates(&name, max_depth);
        }

        for rule in &self.fold_rules {
            name = rule
                .regex
                .replace_all(&name, rule.replace_as.as_str())
                .into_owned();
        }

        let class = self.classify_name(&name).map(str::to_string);

        Frame {
            name,
            class,
            file: raw.file,
            line: raw.line,
        }
    }

    /// Classifies a normalized symbol name: user `classes` rules (first match wins), then built-in rules (spec §12.1 step 4).
    pub fn classify_name(&self, name: &str) -> Option<&str> {
        for rule in &self.class_rules {
            if rule.regex.is_match(name) {
                return Some(rule.class.as_str());
            }
        }
        for builtin in &self.builtin_classes {
            if builtin.regex.is_match(name) {
                return Some(builtin.class);
            }
        }
        None
    }

    /// Classifies a GPU kernel name, defaulting to `"compute"` when no `memcpy`, `sync`, or user class matches (spec §12.1).
    pub fn classify_gpu_kernel(&self, name: &str) -> &str {
        self.classify_name(name).unwrap_or("compute")
    }
}
