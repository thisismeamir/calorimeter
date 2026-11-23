//! Slurm and SSH adapters plus Slurm partition/node discovery parsers (spec §6.2, §7 R-INV-6, §15).

pub mod error;
pub mod slurm;
pub mod slurm_parse;
pub mod ssh;

pub use error::AdapterError;
pub use slurm::{parse_slurm_job_state, SbatchRequest, SlurmAdapter};
pub use slurm_parse::{
    assert_partition_named, group_node_classes, list_partitions_report, parse_scontrol_nodes,
    parse_scontrol_partitions, parse_sinfo_partitions, NodeClassSummary, NodeInfo, PartitionInfo,
};
pub use ssh::{shell_quote, SshAdapter, SshConfig};
