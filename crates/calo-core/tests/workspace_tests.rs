use calo_core::proc::{CommandOutput, CommandRunner, CommandSpec, ProcError, RealCommandRunner};
use calo_core::workspace::{
    check_patchsets, compute_cache_key, execute_build, resolve_sources, BuildTargetContext,
    Worktree,
};
use calo_schema::suite::{
    BuildDef, PatchApplyConfig, PatchConflictPolicy, PatchMetadata, PatchesConfig, RebuildPolicy,
    SourceDef, Suite, SuiteMeta,
};
use calo_schema::{from_yaml_str, SchemaDoc};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_ws_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

struct FakeRunner {
    calls: Mutex<Vec<CommandSpec>>,
    #[allow(clippy::type_complexity)]
    handler: Box<dyn Fn(&CommandSpec) -> Result<CommandOutput, ProcError> + Send + Sync>,
}

impl FakeRunner {
    fn new<F>(f: F) -> Self
    where
        F: Fn(&CommandSpec) -> Result<CommandOutput, ProcError> + Send + Sync + 'static,
    {
        Self {
            calls: Mutex::new(Vec::new()),
            handler: Box::new(f),
        }
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcError> {
        self.calls.lock().unwrap().push(spec.clone());
        (self.handler)(spec)
    }
}

#[test]
fn resolve_sources_pins_rev_via_command_runner() {
    let runner = FakeRunner::new(|spec| {
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args[0], "ls-remote");
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: "0123456789abcdef0123456789abcdef01234567\trefs/tags/v1.0\n".to_string(),
            ..CommandOutput::default()
        })
    });

    let suite: Suite = from_yaml_str(
        r#"
schema: calo.suite/1
suite:
  name: src_test
sources:
  cosmo:
    git: https://example.org/cosmo.git
    rev: v1.0
"#,
    )
    .unwrap();

    let resolved = resolve_sources(&suite, &runner).unwrap();
    assert_eq!(
        resolved.get("cosmo").map(String::as_str),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
}

#[test]
fn real_git_worktree_patches_and_checkout_untouched() {
    let dir = temp_dir("git_wt");
    let user_repo = dir.join("user_checkout");
    let workdir = dir.join("workdir");
    fs::create_dir_all(&user_repo).unwrap();

    let runner = RealCommandRunner;
    // Initialize a real temporary git repo
    let git_in = |args: &[&str]| {
        runner
            .run(&CommandSpec::new("git", args.iter().map(|s| s.to_string()).collect()).with_cwd(&user_repo))
            .unwrap()
    };
    assert!(git_in(&["init"]).success());
    assert!(git_in(&["config", "user.email", "test@example.com"]).success());
    assert!(git_in(&["config", "user.name", "Test"]).success());

    fs::write(user_repo.join("main.txt"), "hello\n").unwrap();
    assert!(git_in(&["add", "main.txt"]).success());
    assert!(git_in(&["commit", "-m", "init"]).success());

    let head_sha = git_in(&["rev-parse", "HEAD"]).stdout.trim().to_string();
    let user_tree_before = git_in(&["rev-parse", "HEAD^{tree}"]).stdout.trim().to_string();

    // Create a valid unified diff patch
    let patch_file = dir.join("add_region.patch");
    fs::write(
        &patch_file,
        "--- a/main.txt\n+++ b/main.txt\n@@ -1 +1,2 @@\n hello\n+calo_region\n",
    )
    .unwrap();

    let src_def = SourceDef {
        git: user_repo.display().to_string(),
        rev: head_sha.clone(),
        submodules: None,
    };

    let wt = Worktree::create(
        "app",
        &src_def,
        &head_sha,
        "regions",
        std::slice::from_ref(&patch_file),
        "local",
        &workdir,
        None,
        &runner,
    )
    .unwrap();

    assert_eq!(wt.patchset.name, "regions");
    assert_eq!(wt.patchset.patches.len(), 1);
    assert_eq!(wt.patchset.patches[0].file, "add_region.patch");
    assert_ne!(wt.patchset.tree_hash, user_tree_before);
    assert_eq!(
        fs::read_to_string(wt.path.join("main.txt")).unwrap(),
        "hello\ncalo_region\n"
    );

    // User checkout MUST remain completely untouched (spec R-SAF-2)
    assert_eq!(
        fs::read_to_string(user_repo.join("main.txt")).unwrap(),
        "hello\n"
    );
    let user_tree_after = git_in(&["rev-parse", "HEAD^{tree}"]).stdout.trim().to_string();
    assert_eq!(user_tree_before, user_tree_after);

    // Test conflict `fail` vs `skip` with a bad patch
    let bad_patch = dir.join("bad.patch");
    fs::write(
        &bad_patch,
        "--- a/nonexistent.txt\n+++ b/nonexistent.txt\n@@ -1 +1 @@\n-x\n+y\n",
    )
    .unwrap();

    let fail_res = Worktree::create(
        "app",
        &src_def,
        &head_sha,
        "bad",
        std::slice::from_ref(&bad_patch),
        "local",
        &workdir,
        Some(&PatchApplyConfig {
            method: Some("git-apply".to_string()),
            strip: Some(1),
            on_conflict: Some(PatchConflictPolicy::Fail),
        }),
        &runner,
    );
    assert!(fail_res.is_err());

    let skip_wt = Worktree::create(
        "app",
        &src_def,
        &head_sha,
        "skip",
        &[bad_patch],
        "local",
        &workdir,
        Some(&PatchApplyConfig {
            method: Some("git-apply".to_string()),
            strip: Some(1),
            on_conflict: Some(PatchConflictPolicy::Skip),
        }),
        &runner,
    )
    .unwrap();
    assert!(skip_wt.patchset.patches.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cache_key_sensitivity_cache_hit_and_nondeterminism_detection() {
    let dir = temp_dir("build");
    let wt_dir = dir.join("wt");
    let cache_root = dir.join("builds");
    fs::create_dir_all(&wt_dir).unwrap();

    let ctx = BuildTargetContext {
        source_shas: BTreeMap::from([("app".to_string(), "aaaa".to_string())]),
        patch_tree_hash: "bbbb".to_string(),
        cpu_model: "EPYC 7763".to_string(),
        gpu_arch: "sm_80".to_string(),
        toolchain: "gcc-12".to_string(),
        mpi_identity: "openmpi-5".to_string(),
    };

    let build = BuildDef {
        run: "echo building".to_string(),
        flags: Some("-O3".to_string()),
        backend: None,
        modules: vec!["gcc/12".to_string()],
        debug_info: None,
        device_lineinfo: None,
        rebuild: Some(RebuildPolicy::Cache),
        measured: None,
        artifacts: vec!["bin/*.out".to_string()],
    };

    let k_base = compute_cache_key(&ctx, &build);
    // Sensitivity to every input
    let mut ctx2 = ctx.clone();
    ctx2.gpu_arch = "gfx90a".to_string();
    assert_ne!(k_base, compute_cache_key(&ctx2, &build));
    let mut ctx3 = ctx.clone();
    ctx3.patch_tree_hash = "cccc".to_string();
    assert_ne!(k_base, compute_cache_key(&ctx3, &build));
    let mut build2 = build.clone();
    build2.flags = Some("-O2".to_string());
    assert_ne!(k_base, compute_cache_key(&ctx, &build2));

    // First build runs and hashes `bin/app.out`
    fs::create_dir_all(wt_dir.join("bin")).unwrap();
    fs::write(wt_dir.join("bin/app.out"), b"binary_v1").unwrap();

    let runner = FakeRunner::new(|spec| {
        assert_eq!(spec.program, "sh");
        assert!(spec.args[1].contains("module load gcc/12"));
        Ok(CommandOutput {
            exit_code: Some(0),
            ..CommandOutput::default()
        })
    });

    let out1 = execute_build(
        "rel",
        &build,
        &ctx,
        &BTreeMap::new(),
        &[],
        &wt_dir,
        &cache_root,
        &runner,
    )
    .unwrap();
    assert!(!out1.cache_hit);
    assert!(out1.artifact_hashes.contains_key("bin/app.out"));
    assert!(out1.warnings.is_empty());

    // Second build under `cache` is a cache hit (runner not invoked again)
    let out2 = execute_build(
        "rel",
        &build,
        &ctx,
        &BTreeMap::new(),
        &[],
        &wt_dir,
        &cache_root,
        &runner,
    )
    .unwrap();
    assert!(out2.cache_hit);
    assert_eq!(runner.calls.lock().unwrap().len(), 1);

    // Forced rebuild with modified artifact emits `build_nondeterministic`
    fs::write(wt_dir.join("bin/app.out"), b"binary_v2_nondeterministic").unwrap();
    let mut fresh_build = build;
    fresh_build.rebuild = Some(RebuildPolicy::Fresh);
    let out3 = execute_build(
        "rel",
        &fresh_build,
        &ctx,
        &BTreeMap::new(),
        &[],
        &wt_dir,
        &cache_root,
        &runner,
    )
    .unwrap();
    assert!(!out3.cache_hit);
    assert_eq!(
        out3.warnings[0].code,
        calo_schema::WarningCode::BuildNondeterministic
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_patchsets_detects_conflicts_and_unsatisfied_requires() {
    let suite = Suite {
        schema: Suite::SCHEMA.to_string(),
        include: vec![],
        suite: SuiteMeta {
            name: "p".to_string(),
            version: None,
            description: None,
        },
        context: vec![],
        sources: BTreeMap::new(),
        patches: Some(PatchesConfig {
            dir: None,
            sets: BTreeMap::from([(
                "bad_set".to_string(),
                vec!["a.patch".to_string(), "b.patch".to_string()],
            )]),
            apply: None,
        }),
        builds: BTreeMap::new(),
        steps: BTreeMap::new(),
        benchmarks: vec![],
        services: BTreeMap::new(),
        symbols: None,
        reports: vec![],
        options: None,
    };

    let meta = BTreeMap::from([(
        "a.patch".to_string(),
        PatchMetadata {
            requires: vec!["kosmic >= 0.3".to_string()],
            conflicts_with: vec!["b.patch".to_string()],
            description: None,
        },
    )]);
    let versions = BTreeMap::from([("kosmic".to_string(), "0.2.1".to_string())]);

    let issues = check_patchsets(&suite, &meta, &versions);
    assert_eq!(issues.len(), 2);
}
