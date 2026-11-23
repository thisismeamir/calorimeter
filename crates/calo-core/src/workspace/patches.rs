//! Patchset metadata (`patchset.yaml`) lint validation (spec §8.2 R-PAT-4).

use calo_schema::suite::{PatchMetadata, Suite};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Lint issue reported by `check_patchsets` (spec §8.2 R-PAT-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintIssue {
    /// Patch set name where the issue occurred.
    pub patchset: String,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// Validates patch sets in `suite` against `patch_meta` (`patchset.yaml`) and `resolved_versions` (spec §8.2 R-PAT-4).
///
/// Checks:
/// 1. Missing patch files under `patches.dir` (when `patches.dir` is set).
/// 2. Pairwise `conflicts_with` within the same patch set.
/// 3. Unsatisfiable `requires` constraints (e.g. `kosmic >= 0.3`) against `resolved_versions`.
pub fn check_patchsets(
    suite: &Suite,
    patch_meta: &BTreeMap<String, PatchMetadata>,
    resolved_versions: &BTreeMap<String, String>,
) -> Vec<LintIssue> {
    let mut issues = Vec::new();
    let Some(patches_cfg) = &suite.patches else {
        return issues;
    };

    let dir_path = patches_cfg.dir.as_deref().map(Path::new);

    for (set_name, patch_list) in &patches_cfg.sets {
        let set_members: BTreeSet<&str> = patch_list.iter().map(String::as_str).collect();

        for patch_file in patch_list {
            // Check missing file on disk if `dir` is configured and exists
            if let Some(base_dir) = dir_path {
                let full_path = base_dir.join(patch_file);
                if !full_path.exists() {
                    issues.push(LintIssue {
                        patchset: set_name.clone(),
                        message: format!(
                            "patch file `{}` not found in `{}`",
                            patch_file,
                            base_dir.display()
                        ),
                    });
                }
            }

            if let Some(meta) = patch_meta.get(patch_file) {
                // Check conflicts within the same patch set
                for conflict in &meta.conflicts_with {
                    if set_members.contains(conflict.as_str()) {
                        issues.push(LintIssue {
                            patchset: set_name.clone(),
                            message: format!(
                                "patch `{patch_file}` conflicts with `{conflict}` in patch set `{set_name}`"
                            ),
                        });
                    }
                }

                // Check version requirements against resolved source versions
                for req in &meta.requires {
                    if let Err(msg) = check_version_req(req, resolved_versions) {
                        issues.push(LintIssue {
                            patchset: set_name.clone(),
                            message: format!("patch `{patch_file}` requirement `{req}` unsatisfied: {msg}"),
                        });
                    }
                }
            }
        }
    }

    issues
}

fn check_version_req(
    req: &str,
    resolved_versions: &BTreeMap<String, String>,
) -> Result<(), String> {
    let operators = [">=", "<=", "==", ">", "<"];
    let mut found_op = None;
    for op in operators {
        if let Some((pkg, ver)) = req.split_once(op) {
            found_op = Some((pkg.trim(), op, ver.trim()));
            break;
        }
    }

    let Some((pkg, op, req_ver)) = found_op else {
        // Bare source/feature name check
        let pkg = req.trim();
        if resolved_versions.contains_key(pkg) {
            return Ok(());
        }
        return Err(format!("source `{pkg}` is not present"));
    };

    let Some(actual_ver) = resolved_versions.get(pkg) else {
        return Err(format!("source `{pkg}` is not present"));
    };

    let cmp = compare_semver_like(actual_ver, req_ver);
    let ok = match op {
        ">=" => cmp.is_ge(),
        "<=" => cmp.is_le(),
        "==" => cmp.is_eq(),
        ">" => cmp.is_gt(),
        "<" => cmp.is_lt(),
        _ => false,
    };

    if ok {
        Ok(())
    } else {
        Err(format!("found `{pkg}` version `{actual_ver}`, required `{op} {req_ver}`"))
    }
}

fn compare_semver_like(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_parts = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| {
                let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect()
    };
    let pa = parse_parts(a);
    let pb = parse_parts(b);
    let max_len = pa.len().max(pb.len());
    for i in 0..max_len {
        let va = pa.get(i).copied().unwrap_or(0);
        let vb = pb.get(i).copied().unwrap_or(0);
        match va.cmp(&vb) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}
