use calo_core::adapters::{
    assert_partition_named, group_node_classes, list_partitions_report, parse_scontrol_nodes,
    parse_scontrol_partitions, parse_sinfo_partitions, parse_slurm_job_state, shell_quote,
    AdapterError, SbatchRequest, SlurmAdapter, SshAdapter, SshConfig,
};
use calo_core::exec::JobState;
use calo_schema::from_yaml_str;
use calo_schema::inventory::Inventory;
use std::path::Path;

#[test]
fn sinfo_and_scontrol_heterogeneous_cluster_parsing() {
    let sinfo_out = r#"
cpu*|10|2-00:00:00|64|256000|epyc,avx2|(null)
gpu|4|1-00:00:00|128|512000|a100,nvlink|gpu:a100:4(S:0-1)
gpu|2|1-00:00:00|64|256000|v100|gpu:v100:2(S:0)
"#;
    let parts = parse_sinfo_partitions(sinfo_out);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "cpu");
    assert!(parts[0].is_default);
    assert_eq!(parts[0].nodes, 10);
    assert_eq!(parts[1].name, "gpu");
    assert_eq!(parts[1].nodes, 6);
    assert_eq!(parts[1].node_classes.len(), 2);

    let scontrol_nodes_out = "\
NodeName=nid001 Arch=x86_64 CoresPerSocket=32 CPUTot=64 RealMemory=256000 Partitions=cpu AvailableFeatures=epyc,avx2 Gres=(null)\n\
NodeName=nid002 Arch=x86_64 CoresPerSocket=32 CPUTot=64 RealMemory=256000 Partitions=cpu AvailableFeatures=avx2,epyc Gres=(null)\n\
NodeName=gpu001 CPUTot=128 RealMemory=512000 Partitions=gpu AvailableFeatures=a100 Gres=gpu:a100:4(S:0-1)\n";
    let nodes = parse_scontrol_nodes(scontrol_nodes_out);
    assert_eq!(nodes.len(), 3);
    let classes = group_node_classes(&nodes);
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0].count, 2); // nid001 and nid002 grouped despite feature order
    assert_eq!(classes[1].count, 1);
    assert_eq!(classes[1].gres.as_deref(), Some("gpu:a100:4"));

    let scontrol_parts_out = "\
PartitionName=gpu AllowAccounts=physics,astro AllowQos=normal Default=NO MaxTime=1-00:00:00 TotalNodes=6\n";
    let sc_parts = parse_scontrol_partitions(scontrol_parts_out);
    assert_eq!(sc_parts[0].allow_accounts, vec!["physics", "astro"]);

    let report = list_partitions_report(&parts, "login.hpc");
    assert!(
        report.contains("calo spec --connect slurm://login.hpc --partition cpu --partition gpu")
    );
}

#[test]
fn r_spc_1_refuses_unnamed_partition() {
    let inv: Inventory = from_yaml_str(
        r#"
schema: calo.inventory/1
machines:
  hpc:
    connect:
      type: slurm
      host: login.hpc
      partitions: [gpu]
"#,
    )
    .unwrap();

    // Named in `--partition` -> ok
    assert!(
        assert_partition_named(&["debug".to_string()], Some(&inv), Some("hpc"), "debug").is_ok()
    );
    // Named in inventory `partitions:` -> ok
    assert!(assert_partition_named(&[], Some(&inv), Some("hpc"), "gpu").is_ok());
    // Unnamed -> refused
    let err = assert_partition_named(&[], Some(&inv), Some("hpc"), "secret_part").unwrap_err();
    assert_eq!(
        err,
        AdapterError::UnnamedPartition {
            partition: "secret_part".to_string()
        }
    );
}

#[test]
fn sbatch_script_golden_shapes_and_slurm_job_states() {
    let single_cpu = SlurmAdapter::render_sbatch_script(
        &SbatchRequest {
            job_name: "calo-base".to_string(),
            partition: Some("cpu".to_string()),
            account: Some("phys".to_string()),
            nodes: 1,
            tasks_per_node: 1,
            cpus_per_task: Some(64),
            gpus_per_task: None,
            gres: None,
            time_limit: Some("00:30:00".to_string()),
            exclusive: true,
            modules: vec!["gcc/13".to_string()],
            output_log: None,
        },
        &["srun probe --rank-aware --runspec runspec.yaml".to_string()],
    );
    assert!(single_cpu.contains("#SBATCH --nodes=1"));
    assert!(single_cpu.contains("#SBATCH --cpus-per-task=64"));
    assert!(single_cpu.contains("#SBATCH --exclusive"));
    assert!(single_cpu.contains("module load gcc/13"));

    let group_gpu = SlurmAdapter::render_group_sbatch_script(
        &SbatchRequest {
            job_name: "calo-grp".to_string(),
            partition: Some("gpu".to_string()),
            account: None,
            nodes: 4,
            tasks_per_node: 4,
            cpus_per_task: Some(16),
            gpus_per_task: Some(1),
            gres: None,
            time_limit: Some("02:00:00".to_string()),
            exclusive: false,
            modules: vec![],
            output_log: None,
        },
        &[
            vec![
                "srun".into(),
                "probe".into(),
                "--runspec".into(),
                "r1.yaml".into(),
            ],
            vec![
                "srun".into(),
                "probe".into(),
                "--runspec".into(),
                "r2.yaml".into(),
            ],
        ],
    );
    assert!(group_gpu.contains("#SBATCH --gpus-per-task=1"));
    assert!(!group_gpu.contains("#SBATCH --exclusive"));
    assert!(group_gpu.contains("srun probe --runspec r1.yaml\nsrun probe --runspec r2.yaml"));

    assert_eq!(
        SlurmAdapter::parse_sbatch_job_id("123456;cluster0\n").unwrap(),
        "123456"
    );

    // JobState parsing across Slurm states
    assert_eq!(parse_slurm_job_state("PENDING"), JobState::Pending);
    assert_eq!(parse_slurm_job_state("RUNNING"), JobState::Running);
    assert_eq!(parse_slurm_job_state("COMPLETING"), JobState::Running);
    assert_eq!(parse_slurm_job_state("COMPLETED"), JobState::Completed);
    assert_eq!(parse_slurm_job_state("TIMEOUT"), JobState::TimedOut);
    assert_eq!(parse_slurm_job_state("CANCELLED+"), JobState::Cancelled);
    assert_eq!(parse_slurm_job_state("PREEMPTED"), JobState::Cancelled);
    assert_eq!(
        parse_slurm_job_state("FAILED"),
        JobState::Failed { code: None }
    );
    assert_eq!(
        parse_slurm_job_state("OUT_OF_MEMORY"),
        JobState::Failed { code: None }
    );
}

#[test]
fn ssh_shell_quote_edge_cases_and_argv() {
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("plain/path-1.yaml"), "plain/path-1.yaml");
    assert_eq!(shell_quote("hello world"), "'hello world'");
    assert_eq!(shell_quote("it's $HOME"), "'it'\\''s $HOME'");
    assert_eq!(shell_quote("line1\nline2"), "'line1\nline2'");

    let ssh = SshAdapter::new(SshConfig::new("user@login.hpc"));
    let stage = ssh.stage_argv(Path::new("/tmp/my file.yaml"), "/scratch/file.yaml");
    assert_eq!(stage[0], "rsync");
    assert!(stage.iter().any(|a| a.contains("ControlMaster=auto")));

    let exec = ssh.exec_argv(&["probe".into(), "--runspec".into(), "/tmp/a b.yaml".into()]);
    assert_eq!(exec[0], "ssh");
    assert!(exec.last().unwrap().contains("'/tmp/a b.yaml'"));
}
