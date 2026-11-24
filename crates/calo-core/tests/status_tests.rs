use calo_core::exec::{
    cancel_runs, collect_status, Adapter, ExecError, JobHandle, JobState, StagedRun,
};
use calo_core::plan::{PinnedRun, Selector};
use calo_core::results::{write_atomic_bytes, ResultsDir};
use calo_core::yaml::{Layer, LayeredBuilder};
use calo_schema::common::{Mode, Status};
use calo_schema::from_yaml_str;
use calo_schema::inventory::Inventory;
use calo_schema::manifest::Manifest;
use calo_schema::suite::{SelectorClause, Suite};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_stat_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

struct FakeAdapter {
    states: Mutex<BTreeMap<String, JobState>>,
    cancelled: Mutex<Vec<String>>,
}

impl FakeAdapter {
    fn new(states: BTreeMap<String, JobState>) -> Self {
        Self {
            states: Mutex::new(states),
            cancelled: Mutex::new(Vec::new()),
        }
    }
}

impl Adapter for FakeAdapter {
    fn stage(&self, staged: StagedRun) -> Result<StagedRun, ExecError> {
        Ok(staged)
    }
    fn submit(&self, staged: &StagedRun) -> Result<JobHandle, ExecError> {
        Ok(JobHandle {
            id: staged.run_id.clone(),
            runspec_path: staged.runspec_path.clone(),
            outputs_dir: PathBuf::new(),
        })
    }
    fn poll(&self, handle: &JobHandle) -> Result<JobState, ExecError> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .get(&handle.id)
            .cloned()
            .unwrap_or(JobState::Pending))
    }
    fn fetch(&self, _handle: &JobHandle, _dest_dir: &Path) -> Result<(), ExecError> {
        Ok(())
    }
    fn cancel(&self, handle: &JobHandle) -> Result<(), ExecError> {
        self.cancelled.lock().unwrap().push(handle.id.clone());
        self.states
            .lock()
            .unwrap()
            .insert(handle.id.clone(), JobState::Cancelled);
        Ok(())
    }
}

#[test]
fn status_transitions_crash_discrepancy_and_selective_cancel() {
    let dir = temp_dir("stat");
    let inv: Inventory = from_yaml_str(
        "schema: calo.inventory/1\nmachines:\n  local:\n    connect: { type: local }\n",
    )
    .unwrap();
    let suite_val =
        calo_schema::yaml::from_str("schema: calo.suite/1\nsuite: { name: s }\n").unwrap();
    let mut lb = LayeredBuilder::new();
    lb.add_layer(Layer::Suite, suite_val);
    let layered = lb.resolve::<Suite>().unwrap();

    let res = ResultsDir::create(&dir, &layered.value, &inv).unwrap();
    let runs = vec![
        PinnedRun {
            run_id: "b1.r0".to_string(),
            benchmark: "b1".to_string(),
            group: None,
            mode: Mode::Measure,
            machine: "local".to_string(),
            params: BTreeMap::new(),
            repeat: 0,
            warmup: false,
            linked_from: None,
        },
        PinnedRun {
            run_id: "b2.r0".to_string(),
            benchmark: "b2".to_string(),
            group: None,
            mode: Mode::Measure,
            machine: "local".to_string(),
            params: BTreeMap::new(),
            repeat: 0,
            warmup: false,
            linked_from: None,
        },
    ];
    res.write_manifest(&layered, &inv, &runs, BTreeMap::new(), "0.1.0")
        .unwrap();

    // b1.r0 has a completed record on disk; b2.r0 is running in adapter;
    // simulate crash discrepancy by making adapter report Failed with no record for b2.r0
    write_atomic_bytes(&res.record_path("b1.r0", None), b"schema: calo.record/2\n").unwrap();

    let mut handles = BTreeMap::new();
    handles.insert(
        "b2.r0".to_string(),
        JobHandle {
            id: "b2.r0".to_string(),
            runspec_path: PathBuf::new(),
            outputs_dir: PathBuf::new(),
        },
    );

    let adapter = FakeAdapter::new(BTreeMap::from([(
        "b2.r0".to_string(),
        JobState::Failed { code: None },
    )]));

    let out = collect_status(&res, &adapter, &handles).unwrap();
    assert_eq!(out.summary.done, 1);
    assert_eq!(out.summary.failed, 1);
    assert!(out.rows[1].discrepancy.is_some());

    // Selective cancel of b2 only; b1 record must remain untouched
    let sel = Selector::Clause(SelectorClause {
        benchmark: Some("b2".to_string()),
        ..SelectorClause::default()
    });
    let rep = cancel_runs(&res, &adapter, &sel, &handles).unwrap();
    assert_eq!(rep.cancelled_runs, vec!["b2.r0".to_string()]);
    assert!(res.record_path("b1.r0", None).exists());

    let m: Manifest = from_yaml_str(&fs::read_to_string(res.manifest_path()).unwrap()).unwrap();
    assert_eq!(m.runs[1].status, Status::Failed);

    let _ = fs::remove_dir_all(&dir);
}
