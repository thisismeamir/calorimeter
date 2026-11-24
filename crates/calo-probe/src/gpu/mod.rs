//! Out-of-process GPU G0 discovery and state sampling (Spec §9.6 B24).

#![deny(unsafe_code)]

#[allow(unsafe_code)]
pub mod ffi;

use std::{io, path::Path, time::Duration};

use calo_schema::{
    common::Warning,
    discovery::ObservedGpu,
    record::{RecordGpu, RecordGpuDevice},
};

use crate::table_writer::{TableWriter, WriterHandle};

/// Discovers visible GPUs via dynamic vendor libraries (`dlopen`); returns empty if none.
pub fn discover_gpus() -> Vec<ObservedGpu> {
    ffi::NvmlHandle::try_load()
        .map(|h| h.discover_devices())
        .unwrap_or_default()
}

struct DeviceSampler {
    index: u32,
    writer: WriterHandle,
    last_t_ns: Option<i64>,
    last_power_w: Option<f64>,
    energy_j: f64,
    has_energy: bool,
}

/// Summary returned when finalizing [`GpuStateCollector`].
pub struct GpuStateSummary {
    pub record_gpu: RecordGpu,
    pub total_energy_j: Option<f64>,
    pub warnings: Vec<Warning>,
}

/// Manages G0 out-of-process NVML polling and `gpu.state.dev<k>.calotbl` writers.
pub struct GpuStateCollector {
    nvml: ffi::NvmlHandle,
    devices: Vec<DeviceSampler>,
    owner_rank: u32,
}

impl GpuStateCollector {
    /// Opens NVML and spawns one `gpu.state` writer per visible device.
    /// Returns `Ok(None)` if NVML is unavailable or zero GPUs are visible.
    pub fn open(out_dir: &Path, owner_rank: u32) -> io::Result<Option<Self>> {
        let Some(nvml) = ffi::NvmlHandle::try_load() else {
            return Ok(None);
        };
        let count = nvml.device_count();
        if count == 0 {
            return Ok(None);
        }

        let mut devices = Vec::with_capacity(count as usize);
        for index in 0..count {
            let rel = format!("gpu.state.dev{index}.calotbl");
            let path = out_dir.join(&rel);
            let writer =
                WriterHandle::spawn(TableWriter::create_gpu_state(&path, &rel, index)?, 2048);
            devices.push(DeviceSampler {
                index,
                writer,
                last_t_ns: None,
                last_power_w: None,
                energy_j: 0.0,
                has_energy: false,
            });
        }

        Ok(Some(Self {
            nvml,
            devices,
            owner_rank,
        }))
    }

    /// Samples all managed GPUs at monotonic timestamp `t_ns` and integrates power into Joules.
    pub fn sample_all(&mut self, t_ns: i64) {
        for dev in &mut self.devices {
            if let Some(row) = self.nvml.sample_device(dev.index, t_ns) {
                if row.power_w.is_finite() {
                    let p = f64::from(row.power_w);
                    if let (Some(prev_t), Some(prev_p)) = (dev.last_t_ns, dev.last_power_w) {
                        let dt_s = (t_ns.saturating_sub(prev_t) as f64) * 1e-9;
                        if dt_s > 0.0 {
                            dev.energy_j += 0.5 * (prev_p + p) * dt_s;
                            dev.has_energy = true;
                        }
                    }
                    dev.last_t_ns = Some(t_ns);
                    dev.last_power_w = Some(p);
                }
                dev.writer.send_gpu_state(row);
            }
        }
    }

    /// Flushes and closes all GPU state sidecar writers.
    pub fn finish(self) -> io::Result<GpuStateSummary> {
        let mut record_devices = Vec::with_capacity(self.devices.len());
        let mut warnings = Vec::new();
        let mut total_energy = 0.0f64;
        let mut any_energy = false;

        for dev in self.devices {
            let _ = dev.writer.flush_now_blocking(Duration::from_secs(1));
            let out = dev.writer.finish()?;
            warnings.extend(out.warnings);
            if dev.has_energy {
                total_energy += dev.energy_j;
                any_energy = true;
            }
            record_devices.push(RecordGpuDevice {
                index: dev.index,
                owners: vec![self.owner_rank],
                state_series: Some(out.sidecar),
                kernels: None,
                memcpy: None,
                timeline: None,
                counters: None,
                clock_correlation: None,
            });
        }

        Ok(GpuStateSummary {
            record_gpu: RecordGpu {
                devices: record_devices,
                source: None,
                tracing_overhead_s: None,
            },
            total_energy_j: any_energy.then_some(total_energy),
            warnings,
        })
    }
}
