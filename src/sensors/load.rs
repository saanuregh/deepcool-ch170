//! Total CPU load %, from the Win32 `GetSystemTimes` cumulative counters.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;

pub struct CpuLoad {
    last: Option<(u64, u64, u64)>, // idle, kernel, user
    last_load: f64,
}

impl CpuLoad {
    pub fn new() -> Self {
        Self {
            last: None,
            last_load: 0.0,
        }
    }

    /// Total CPU load %, computed from two samples over the call interval.
    pub fn usage(&mut self) -> f64 {
        let times = match read_system_times() {
            Some(t) => t,
            None => return self.last_load,
        };

        let load = match self.last {
            Some((li, lk, lu)) => {
                let idle_d = times.0.saturating_sub(li) as f64;
                let kernel_d = times.1.saturating_sub(lk) as f64; // includes idle
                let user_d = times.2.saturating_sub(lu) as f64;
                let total = kernel_d + user_d;
                if total > 0.0 {
                    ((total - idle_d) / total * 100.0).clamp(0.0, 100.0)
                } else {
                    self.last_load
                }
            }
            None => 0.0,
        };

        self.last = Some(times);
        self.last_load = load;
        load
    }
}

fn filetime_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

/// (idle, kernel, user) cumulative 100-ns system times, or None on failure.
fn read_system_times() -> Option<(u64, u64, u64)> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: three valid, initialized FILETIME out-params.
    let ok = unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) };
    ok.ok()?;
    Some((
        filetime_to_u64(idle),
        filetime_to_u64(kernel),
        filetime_to_u64(user),
    ))
}
