//! Git source revision pinning (`resolve_sources`, spec §8.1 R-SRC-1).

use crate::proc::{CommandRunner, CommandSpec};
use crate::workspace::error::WorkspaceError;
use calo_schema::suite::Suite;
use std::collections::BTreeMap;
use std::path::Path;

/// Resolves every `sources.<name>.rev` in `suite` to a concrete 40-char commit SHA (spec §8.1 R-SRC-1).
pub fn resolve_sources(
    suite: &Suite,
    runner: &dyn CommandRunner,
) -> Result<BTreeMap<String, String>, WorkspaceError> {
    let mut resolved = BTreeMap::new();

    for (name, src) in &suite.sources {
        let sha = resolve_single_rev(name, &src.git, &src.rev, runner)?;
        resolved.insert(name.clone(), sha);
    }

    Ok(resolved)
}

fn resolve_single_rev(
    name: &str,
    git_url: &str,
    rev: &str,
    runner: &dyn CommandRunner,
) -> Result<String, WorkspaceError> {
    if is_full_sha(rev) {
        return Ok(rev.to_ascii_lowercase());
    }

    // Try `git ls-remote <git> <rev>` first (works for tags, branches, and remote URLs)
    let ls_spec = CommandSpec::new(
        "git",
        vec![
            "ls-remote".to_string(),
            git_url.to_string(),
            rev.to_string(),
        ],
    );
    let ls_out = runner.run(&ls_spec)?;
    if ls_out.success() {
        if let Some(first_line) = ls_out.stdout.lines().find(|l| !l.trim().is_empty()) {
            if let Some(sha) = first_line.split_whitespace().next() {
                if is_full_sha(sha) {
                    return Ok(sha.to_ascii_lowercase());
                }
            }
        }
    }

    // Fallback for local repositories or abbreviated SHAs: `git -C <git> rev-parse <rev>^{commit}`
    if Path::new(git_url).exists() {
        let rp_spec = CommandSpec::new(
            "git",
            vec![
                "-C".to_string(),
                git_url.to_string(),
                "rev-parse".to_string(),
                format!("{rev}^{{commit}}"),
            ],
        );
        let rp_out = runner.run(&rp_spec)?;
        if rp_out.success() {
            let sha = rp_out.stdout.trim();
            if is_full_sha(sha) {
                return Ok(sha.to_ascii_lowercase());
            }
        }
    }

    Err(WorkspaceError::SourceResolution {
        source_name: name.to_string(),
        rev: rev.to_string(),
        reason: ls_out.stderr.trim().to_string(),
    })
}

fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}
