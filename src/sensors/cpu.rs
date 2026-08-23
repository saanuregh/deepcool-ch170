//! AMD Zen (family 17h/19h/1Ah) CPU sensors via PawnIO: temperature (SMN),
//! package power (RAPL MSRs), and average effective clock (per-core APERF).
//! The register map and math mirror LibreHardwareMonitor's `Amd17Cpu`.

use crate::pawnio::PawnIoModule;
use anyhow::Result;
use std::time::Instant;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};

// SMN: SMU thermal current temperature (Tctl), stable across Zen families.
const F17H_M01H_THM_TCON_CUR_TMP: u32 = 0x0005_9800;
const F17H_TEMP_RANGE_SEL_MASK: u32 = 0x8_0000; // bit 19
const F17H_TEMP_TJ_SEL_MASK: u32 = 0x3_0000; // bits [17:16]

// RAPL energy MSRs.
const MSR_PWR_UNIT: u32 = 0xC001_0299;
const MSR_PKG_ENERGY_STAT: u32 = 0xC001_029B;

// Effective-clock counter (read per logical processor under thread affinity).
const MSR_APERF_RO: u32 = 0xC000_00E8;

// APERF delta sanity ceiling (LHM uses 20000e6 to reject counter resets).
const APERF_DELTA_MAX: f64 = 20_000e6;

pub struct CpuSensors {
    amd: PawnIoModule,
    num_cpus: usize,

    last_pkg_energy: Option<u32>,
    last_pkg_time: Instant,
    last_power: f64,
    energy_unit: Option<f64>, // RAPL joules-per-count; a hardware constant, read once

    last_aperf: Vec<u64>,
    last_clock_time: Instant,
    clock_primed: bool,
    last_freq: f64,
}

impl CpuSensors {
    pub fn new(amd: PawnIoModule) -> Self {
        let num_cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let now = Instant::now();
        Self {
            amd,
            num_cpus,
            last_pkg_energy: None,
            last_pkg_time: now,
            last_power: 0.0,
            energy_unit: None,
            last_aperf: vec![0; num_cpus],
            last_clock_time: now,
            clock_primed: false,
            last_freq: 0.0,
        }
    }

    /// Tctl/Tdie in °C from the SMU thermal register.
    pub fn temperature(&self) -> Result<f64> {
        let raw = self.amd.read_smn(F17H_M01H_THM_TCON_CUR_TMP)?;
        // bits [31:21] in steps of 0.125 °C.
        let mut t = ((raw >> 21) as f64) * 0.125;
        let offset_flag = (raw & F17H_TEMP_RANGE_SEL_MASK) != 0
            || (raw & F17H_TEMP_TJ_SEL_MASK) == F17H_TEMP_TJ_SEL_MASK;
        if offset_flag {
            t -= 49.0;
        }
        // NOTE: some Zen SKUs carry an extra per-model Tctl offset; the 9800X3D
        // (family 1Ah) uses none, matching LHM's "Core (Tctl/Tdie)".
        Ok(t)
    }

    /// Package power in watts, from RAPL energy-counter deltas over wall-clock time.
    pub fn package_power(&mut self) -> Result<f64> {
        let now = Instant::now();

        // The energy unit is a fixed hardware constant; read it only once.
        let energy_unit = match self.energy_unit {
            Some(u) => u,
            None => {
                let esu = (self.amd.read_msr(MSR_PWR_UNIT)? as u32 >> 8) & 0x1F;
                let u = 0.5f64.powi(esu as i32); // joules per count
                self.energy_unit = Some(u);
                u
            }
        };

        let total = self.amd.read_msr(MSR_PKG_ENERGY_STAT)? as u32;

        let power = match self.last_pkg_energy {
            Some(last) => {
                // 32-bit counter; wrapping_sub gives the increment count across a wrap.
                let delta = u64::from(total.wrapping_sub(last));
                let secs = now.duration_since(self.last_pkg_time).as_secs_f64();
                if secs > 0.0 {
                    energy_unit * delta as f64 / secs
                } else {
                    self.last_power
                }
            }
            None => 0.0, // first sample: no delta yet
        };

        self.last_pkg_energy = Some(total);
        self.last_pkg_time = now;
        self.last_power = power;
        Ok(power)
    }

    /// Average effective core clock in MHz: mean over logical processors of
    /// (ΔAPERF / Δwall-clock). APERF only ticks in C0, so this reflects real
    /// utilization-weighted frequency (LHM's "Cores (Average Effective)").
    pub fn effective_clock_mhz(&mut self) -> Result<f64> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_clock_time).as_secs_f64();

        let mut current = vec![0u64; self.num_cpus];
        for (cpu, slot) in current.iter_mut().enumerate() {
            *slot = self.read_msr_on_cpu(cpu, MSR_APERF_RO)?;
        }

        let freq = if self.clock_primed && elapsed > 0.0 {
            let mut sum = 0.0;
            let mut n = 0u32;
            for (&cur, &last) in current.iter().zip(self.last_aperf.iter()) {
                let delta = cur.wrapping_sub(last) as f64;
                if delta < APERF_DELTA_MAX {
                    // cycles / seconds / 1e6 = MHz
                    sum += delta / elapsed / 1_000_000.0;
                    n += 1;
                }
            }
            if n > 0 {
                sum / n as f64
            } else {
                self.last_freq
            }
        } else {
            0.0
        };

        self.last_aperf = current;
        self.last_clock_time = now;
        self.clock_primed = true;
        self.last_freq = freq;
        Ok(freq)
    }

    /// Read an MSR pinned to a specific logical processor (MSRs are per-core).
    fn read_msr_on_cpu(&self, cpu: usize, msr: u32) -> Result<u64> {
        // SAFETY: pseudo-handle from GetCurrentThread needs no close; mask fits
        // one processor group (<= 64 logical CPUs). Affinity is restored after.
        unsafe {
            let thread = GetCurrentThread();
            let prev = SetThreadAffinityMask(thread, 1usize << cpu);
            let result = self.amd.read_msr(msr);
            if prev != 0 {
                SetThreadAffinityMask(thread, prev);
            }
            result
        }
    }
}
