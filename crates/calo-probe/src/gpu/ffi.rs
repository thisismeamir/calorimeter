//! Dynamic NVML (`libnvidia-ml.so.1`) FFI bindings via `libc::dlopen` (Spec §3 I1, I8, §9.6 B24).

use std::ffi::{c_char, c_int, c_uint, c_ulonglong, c_void, CStr};

use calo_schema::{
    discovery::{ObservedGpu, ObservedGpuClocks},
    tables::GpuStateRow,
};

const NVML_SUCCESS: c_int = 0;
const NVML_CLOCK_SM: c_uint = 1;
const NVML_CLOCK_MEM: c_uint = 2;
const NVML_TEMPERATURE_GPU: c_uint = 0;

#[repr(C)]
struct NvmlMemory {
    total: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

#[repr(C)]
struct NvmlUtilization {
    gpu: c_uint,
    memory: c_uint,
}

type NvmlDevice = *mut c_void;

type FnInit = unsafe extern "C" fn() -> c_int;
type FnShutdown = unsafe extern "C" fn() -> c_int;
type FnDriverVersion = unsafe extern "C" fn(*mut c_char, c_uint) -> c_int;
type FnDeviceCount = unsafe extern "C" fn(*mut c_uint) -> c_int;
type FnDeviceHandle = unsafe extern "C" fn(c_uint, *mut NvmlDevice) -> c_int;
type FnDeviceName = unsafe extern "C" fn(NvmlDevice, *mut c_char, c_uint) -> c_int;
type FnMemoryInfo = unsafe extern "C" fn(NvmlDevice, *mut NvmlMemory) -> c_int;
type FnUtilization = unsafe extern "C" fn(NvmlDevice, *mut NvmlUtilization) -> c_int;
type FnPowerUsage = unsafe extern "C" fn(NvmlDevice, *mut c_uint) -> c_int;
type FnClockInfo = unsafe extern "C" fn(NvmlDevice, c_uint, *mut c_uint) -> c_int;
type FnTemperature = unsafe extern "C" fn(NvmlDevice, c_uint, *mut c_uint) -> c_int;
type FnThrottle = unsafe extern "C" fn(NvmlDevice, *mut c_ulonglong) -> c_int;
type FnComputeCap = unsafe extern "C" fn(NvmlDevice, *mut c_int, *mut c_int) -> c_int;

/// Dynamically loaded NVML handle. Calls `nvmlShutdown` and `dlclose` on drop.
pub struct NvmlHandle {
    lib: *mut c_void,
    shutdown: FnShutdown,
    driver_version: Option<FnDriverVersion>,
    device_count: FnDeviceCount,
    device_handle: FnDeviceHandle,
    device_name: Option<FnDeviceName>,
    memory_info: Option<FnMemoryInfo>,
    utilization: Option<FnUtilization>,
    power_usage: Option<FnPowerUsage>,
    clock_info: Option<FnClockInfo>,
    temperature: Option<FnTemperature>,
    throttle: Option<FnThrottle>,
    compute_cap: Option<FnComputeCap>,
}

// SAFETY: NVML functions are thread-safe after `nvmlInit_v2` and `NvmlHandle`
// only exposes `&self` read-only queries before `Drop`.
unsafe impl Send for NvmlHandle {}

/// Looks up a symbol `name` (NUL-terminated) in `lib` and transmutes it to `T`.
///
/// # Safety
/// `lib` must be a valid `dlopen` handle and `T` must match the exact C ABI signature of `name`.
unsafe fn load_sym<T: Copy>(lib: *mut c_void, name: &'static CStr) -> Option<T> {
    let ptr = libc::dlsym(lib, name.as_ptr());
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&ptr))
    }
}

impl NvmlHandle {
    /// Attempts to `dlopen` `libnvidia-ml.so.1` (fallback `libnvidia-ml.so`) and initialize NVML.
    /// Returns `None` cleanly if NVML is absent or initialization fails (Spec §3 I1).
    pub fn try_load() -> Option<Self> {
        let candidates = [c"libnvidia-ml.so.1", c"libnvidia-ml.so"];
        let mut lib: *mut c_void = std::ptr::null_mut();
        for cand in candidates {
            // SAFETY: `cand` is a valid NUL-terminated static CStr.
            lib = unsafe { libc::dlopen(cand.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if !lib.is_null() {
                break;
            }
        }
        if lib.is_null() {
            return None;
        }

        // SAFETY: All symbol signatures match NVIDIA NVML v2 C headers.
        unsafe {
            let Some(init): Option<FnInit> = load_sym(lib, c"nvmlInit_v2") else {
                libc::dlclose(lib);
                return None;
            };
            let Some(shutdown): Option<FnShutdown> = load_sym(lib, c"nvmlShutdown") else {
                libc::dlclose(lib);
                return None;
            };
            let Some(device_count): Option<FnDeviceCount> = load_sym(lib, c"nvmlDeviceGetCount_v2")
            else {
                libc::dlclose(lib);
                return None;
            };
            let Some(device_handle): Option<FnDeviceHandle> =
                load_sym(lib, c"nvmlDeviceGetHandleByIndex_v2")
            else {
                libc::dlclose(lib);
                return None;
            };

            if init() != NVML_SUCCESS {
                libc::dlclose(lib);
                return None;
            }

            Some(Self {
                lib,
                shutdown,
                driver_version: load_sym(lib, c"nvmlSystemGetDriverVersion"),
                device_count,
                device_handle,
                device_name: load_sym(lib, c"nvmlDeviceGetName"),
                memory_info: load_sym(lib, c"nvmlDeviceGetMemoryInfo"),
                utilization: load_sym(lib, c"nvmlDeviceGetUtilizationRates"),
                power_usage: load_sym(lib, c"nvmlDeviceGetPowerUsage"),
                clock_info: load_sym(lib, c"nvmlDeviceGetClockInfo"),
                temperature: load_sym(lib, c"nvmlDeviceGetTemperature"),
                throttle: load_sym(lib, c"nvmlDeviceGetCurrentClocksThrottleReasons"),
                compute_cap: load_sym(lib, c"nvmlDeviceGetCudaComputeCapability"),
            })
        }
    }

    /// Returns the number of visible NVIDIA GPUs.
    pub fn device_count(&self) -> u32 {
        let mut count: c_uint = 0;
        // SAFETY: `self` holds an initialized NVML session and valid out pointer.
        if unsafe { (self.device_count)(&mut count) } == NVML_SUCCESS {
            count
        } else {
            0
        }
    }

    fn get_device(&self, index: u32) -> Option<NvmlDevice> {
        let mut dev: NvmlDevice = std::ptr::null_mut();
        // SAFETY: `self` is initialized and `dev` is a valid out pointer.
        if unsafe { (self.device_handle)(index, &mut dev) } == NVML_SUCCESS && !dev.is_null() {
            Some(dev)
        } else {
            None
        }
    }

    /// Queries static hardware descriptors for all visible NVIDIA GPUs (Spec §6.3).
    pub fn discover_devices(&self) -> Vec<ObservedGpu> {
        let driver = self.driver_version.and_then(|f| {
            let mut buf = [0u8; 96];
            // SAFETY: `buf` is 96 writable bytes; NVML NUL-terminates within length.
            let rc = unsafe { f(buf.as_mut_ptr().cast::<c_char>(), buf.len() as c_uint) };
            (rc == NVML_SUCCESS).then(|| c_buf_to_string(&buf))
        });

        let count = self.device_count();
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count {
            let Some(dev) = self.get_device(index) else {
                continue;
            };
            let model = self.device_name.and_then(|f| {
                let mut buf = [0u8; 128];
                // SAFETY: `dev` is valid and `buf` is 128 writable bytes.
                let rc = unsafe { f(dev, buf.as_mut_ptr().cast::<c_char>(), buf.len() as c_uint) };
                (rc == NVML_SUCCESS).then(|| c_buf_to_string(&buf))
            });
            let arch = self.compute_cap.and_then(|f| {
                let (mut major, mut minor) = (0 as c_int, 0 as c_int);
                // SAFETY: `dev` and out pointers are valid.
                let rc = unsafe { f(dev, &mut major, &mut minor) };
                (rc == NVML_SUCCESS).then(|| format!("sm_{major}{minor}"))
            });
            let mem_bytes = self.memory_info.and_then(|f| {
                let mut mem = NvmlMemory {
                    total: 0,
                    free: 0,
                    used: 0,
                };
                // SAFETY: `dev` and `mem` are valid.
                let rc = unsafe { f(dev, &mut mem) };
                (rc == NVML_SUCCESS).then_some(mem.total)
            });
            let sm_mhz = self.clock_info.and_then(|f| {
                let mut mhz: c_uint = 0;
                // SAFETY: `dev` and `mhz` are valid.
                (unsafe { f(dev, NVML_CLOCK_SM, &mut mhz) } == NVML_SUCCESS).then_some(mhz)
            });
            let mem_mhz = self.clock_info.and_then(|f| {
                let mut mhz: c_uint = 0;
                // SAFETY: `dev` and `mhz` are valid.
                (unsafe { f(dev, NVML_CLOCK_MEM, &mut mhz) } == NVML_SUCCESS).then_some(mhz)
            });
            let clocks = (sm_mhz.is_some() || mem_mhz.is_some())
                .then_some(ObservedGpuClocks { sm_mhz, mem_mhz });

            out.push(ObservedGpu {
                index,
                vendor: Some("nvidia".to_string()),
                model,
                arch,
                sms: None,
                mem_bytes,
                driver: driver.clone(),
                runtime: None,
                ecc: None,
                clocks,
                link: Vec::new(),
                numa: None,
                profiling_permitted: None,
            });
        }
        out
    }

    /// Samples one G0 state row for GPU `index` at monotonic timestamp `t_ns` (Spec §9.6 B24).
    pub fn sample_device(&self, index: u32, t_ns: i64) -> Option<GpuStateRow> {
        let dev = self.get_device(index)?;

        let util_pct = self
            .utilization
            .and_then(|f| {
                let mut u = NvmlUtilization { gpu: 0, memory: 0 };
                // SAFETY: `dev` and `u` are valid.
                (unsafe { f(dev, &mut u) } == NVML_SUCCESS).then_some(u.gpu as f32)
            })
            .unwrap_or(f32::NAN);

        let mem_used_bytes = self
            .memory_info
            .and_then(|f| {
                let mut m = NvmlMemory {
                    total: 0,
                    free: 0,
                    used: 0,
                };
                // SAFETY: `dev` and `m` are valid.
                (unsafe { f(dev, &mut m) } == NVML_SUCCESS).then_some(m.used as i64)
            })
            .unwrap_or(0);

        let power_w = self
            .power_usage
            .and_then(|f| {
                let mut mw: c_uint = 0;
                // SAFETY: `dev` and `mw` are valid.
                (unsafe { f(dev, &mut mw) } == NVML_SUCCESS).then_some(mw as f32 / 1000.0)
            })
            .unwrap_or(f32::NAN);

        let sm_mhz = self
            .clock_info
            .and_then(|f| {
                let mut mhz: c_uint = 0;
                // SAFETY: `dev` and `mhz` are valid.
                (unsafe { f(dev, NVML_CLOCK_SM, &mut mhz) } == NVML_SUCCESS).then_some(mhz as i32)
            })
            .unwrap_or(0);

        let mem_mhz = self
            .clock_info
            .and_then(|f| {
                let mut mhz: c_uint = 0;
                // SAFETY: `dev` and `mhz` are valid.
                (unsafe { f(dev, NVML_CLOCK_MEM, &mut mhz) } == NVML_SUCCESS).then_some(mhz as i32)
            })
            .unwrap_or(0);

        let temp_c = self
            .temperature
            .and_then(|f| {
                let mut t: c_uint = 0;
                // SAFETY: `dev` and `t` are valid.
                (unsafe { f(dev, NVML_TEMPERATURE_GPU, &mut t) } == NVML_SUCCESS)
                    .then_some(t as f32)
            })
            .unwrap_or(f32::NAN);

        let throttle_mask = self
            .throttle
            .and_then(|f| {
                let mut mask: c_ulonglong = 0;
                // SAFETY: `dev` and `mask` are valid.
                (unsafe { f(dev, &mut mask) } == NVML_SUCCESS).then_some(mask as i64)
            })
            .unwrap_or(0);

        Some(GpuStateRow {
            t_ns,
            util_pct,
            mem_used_bytes,
            power_w,
            sm_mhz,
            mem_mhz,
            temp_c,
            throttle_mask,
            link_tx_bytes: 0,
            link_rx_bytes: 0,
        })
    }
}

impl Drop for NvmlHandle {
    fn drop(&mut self) {
        // SAFETY: `nvmlInit_v2` succeeded in `try_load`; `lib` is a valid `dlopen` handle.
        unsafe {
            let _ = (self.shutdown)();
            libc::dlclose(self.lib);
        }
    }
}

fn c_buf_to_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}
