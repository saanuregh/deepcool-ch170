//! Minimal safe wrapper over `PawnIOLib.dll`, the user-mode entry point to the
//! PawnIO kernel driver. We use it to read MSRs / SMN registers (AMD CPU) and to
//! do SuperIO port I/O (motherboard fan chip) without LibreHardwareMonitor.
//!
//! PawnIO must be installed (the signed driver + this DLL) and the process must
//! run elevated — the driver rejects `pawnio_open` from non-admin callers.
//!
//! The DLL is `extern "C"` / `__stdcall` (== `extern "system"`) returning HRESULT.

use anyhow::{Context, Result, bail};
use std::ffi::{CStr, c_void};
use std::os::raw::c_char;
use std::sync::Arc;

type Handle = *mut c_void;

type FnVersion = unsafe extern "system" fn(*mut u32) -> i32;
type FnOpen = unsafe extern "system" fn(*mut Handle) -> i32;
type FnLoad = unsafe extern "system" fn(Handle, *const u8, usize) -> i32;
type FnExecute = unsafe extern "system" fn(
    Handle,
    *const c_char, // function name (PCSTR, null-terminated)
    *const u64,    // input buffer
    usize,         // input element count
    *mut u64,      // output buffer
    usize,         // output capacity (elements)
    *mut usize,    // out: elements written
) -> i32;
type FnClose = unsafe extern "system" fn(Handle) -> i32;

const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;

/// Candidate locations for PawnIOLib.dll. The installer puts it under
/// `%ProgramFiles%\PawnIO`; we also try the bare name in case it is on PATH.
fn dll_candidates() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        v.push(format!(r"{pf}\PawnIO\PawnIOLib.dll"));
    }
    v.push(r"C:\Program Files\PawnIO\PawnIOLib.dll".to_string());
    v.push("PawnIOLib.dll".to_string());
    v
}

fn hr_failed(hr: i32) -> bool {
    hr < 0
}

/// The loaded DLL plus its resolved function pointers. Kept behind an `Arc` so
/// multiple modules can share one DLL load. The `_lib` field must outlive the
/// function pointers, which it does as a sibling field.
struct Lib {
    _lib: libloading::Library,
    version: FnVersion,
    open: FnOpen,
    load: FnLoad,
    execute: FnExecute,
    close: FnClose,
}

impl Lib {
    fn load() -> Result<Arc<Self>> {
        let mut last_err = None;
        for path in dll_candidates() {
            match unsafe { libloading::Library::new(&path) } {
                Ok(lib) => return Self::from_library(lib),
                Err(e) => last_err = Some((path, e)),
            }
        }
        let (path, e) = last_err.expect("at least one candidate");
        Err(anyhow::anyhow!(e)).with_context(|| {
            format!(
                "Failed to load PawnIOLib.dll (last tried {path}). \
                 PawnIO is required — install it from https://pawnio.eu \
                 (winget install namazso.PawnIO)."
            )
        })
    }

    fn from_library(lib: libloading::Library) -> Result<Arc<Self>> {
        // SAFETY: names match the exported symbols; signatures match PawnIOLib.h.
        // Dereferencing the Symbol copies the fn pointer out; it stays valid as
        // long as `_lib` (stored alongside) is alive.
        unsafe {
            let version = *lib
                .get::<FnVersion>(b"pawnio_version\0")
                .context("pawnio_version")?;
            let open = *lib.get::<FnOpen>(b"pawnio_open\0").context("pawnio_open")?;
            let load = *lib.get::<FnLoad>(b"pawnio_load\0").context("pawnio_load")?;
            let execute = *lib
                .get::<FnExecute>(b"pawnio_execute\0")
                .context("pawnio_execute")?;
            let close = *lib
                .get::<FnClose>(b"pawnio_close\0")
                .context("pawnio_close")?;
            Ok(Arc::new(Self {
                _lib: lib,
                version,
                open,
                load,
                execute,
                close,
            }))
        }
    }
}

/// Handle to the PawnIO library. Load modules from it with [`PawnIo::load_module`].
pub struct PawnIo {
    lib: Arc<Lib>,
}

impl PawnIo {
    pub fn open() -> Result<Self> {
        Ok(Self { lib: Lib::load()? })
    }

    /// Returns `(major, minor, patch)`.
    pub fn version(&self) -> (u8, u8, u8) {
        let mut v: u32 = 0;
        unsafe { (self.lib.version)(&mut v) };
        (
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        )
    }

    /// Open an executor and load a compiled Pawn module blob into it.
    pub fn load_module(&self, blob: &[u8]) -> Result<PawnIoModule> {
        let mut handle: Handle = std::ptr::null_mut();
        let hr = unsafe { (self.lib.open)(&mut handle) };
        if hr_failed(hr) {
            if hr == E_ACCESSDENIED {
                bail!(
                    "PawnIO access denied (E_ACCESSDENIED). This program must run as Administrator."
                );
            }
            bail!("pawnio_open failed: hr=0x{hr:08x}");
        }

        let hr = unsafe { (self.lib.load)(handle, blob.as_ptr(), blob.len()) };
        if hr_failed(hr) {
            unsafe { (self.lib.close)(handle) };
            bail!("pawnio_load failed: hr=0x{hr:08x} (module rejected?)");
        }

        Ok(PawnIoModule {
            lib: self.lib.clone(),
            handle,
        })
    }
}

/// A loaded PawnIO module (one executor + one blob). Not `Send`/`Sync`: use from
/// a single thread. Closes its executor on drop.
pub struct PawnIoModule {
    lib: Arc<Lib>,
    handle: Handle,
}

impl PawnIoModule {
    /// Call a module function that returns a single `u64` (allocation-free).
    fn call_scalar(&self, name: &CStr, input: &[u64]) -> Result<u64> {
        let mut out = 0u64;
        let mut written: usize = 0;
        let hr = unsafe {
            (self.lib.execute)(
                self.handle,
                name.as_ptr(),
                input.as_ptr(),
                input.len(),
                &mut out,
                1,
                &mut written,
            )
        };
        if hr_failed(hr) {
            bail!("pawnio_execute({name:?}) failed: hr=0x{hr:08x}");
        }
        if written < 1 {
            bail!("pawnio_execute({name:?}) returned no data");
        }
        Ok(out)
    }

    /// Call a module function with no output (a write / action).
    fn call_action(&self, name: &CStr, input: &[u64]) -> Result<()> {
        let mut written: usize = 0;
        let hr = unsafe {
            (self.lib.execute)(
                self.handle,
                name.as_ptr(),
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                0,
                &mut written,
            )
        };
        if hr_failed(hr) {
            bail!("pawnio_execute({name:?}) failed: hr=0x{hr:08x}");
        }
        Ok(())
    }

    /// Read a 64-bit MSR.
    pub fn read_msr(&self, index: u32) -> Result<u64> {
        self.call_scalar(c"ioctl_read_msr", &[u64::from(index)])
    }

    /// Read a 32-bit SMN (System Management Network) register.
    pub fn read_smn(&self, offset: u32) -> Result<u32> {
        self.call_scalar(c"ioctl_read_smn", &[u64::from(offset)])
            .map(|v| v as u32)
    }

    /// Read a byte from an x86 I/O port.
    pub fn pio_inb(&self, port: u16) -> Result<u8> {
        self.call_scalar(c"ioctl_pio_inb", &[u64::from(port)])
            .map(|v| v as u8)
    }

    /// Write a byte to an x86 I/O port.
    pub fn pio_outb(&self, port: u16, value: u8) -> Result<()> {
        self.call_action(c"ioctl_pio_outb", &[u64::from(port), u64::from(value)])
    }

    /// Read a SuperIO configuration register (via the 0x2E/0x2F index/data pair).
    pub fn superio_inb(&self, register: u8) -> Result<u8> {
        self.call_scalar(c"ioctl_superio_inb", &[u64::from(register)])
            .map(|v| v as u8)
    }

    /// Read a 16-bit SuperIO configuration register pair (`register`,`register+1`).
    pub fn superio_inw(&self, register: u8) -> Result<u16> {
        self.call_scalar(c"ioctl_superio_inw", &[u64::from(register)])
            .map(|v| v as u16)
    }

    /// Write a SuperIO configuration register.
    pub fn superio_outb(&self, register: u8, value: u8) -> Result<()> {
        self.call_action(
            c"ioctl_superio_outb",
            &[u64::from(register), u64::from(value)],
        )
    }

    /// Select which SuperIO config port pair the module targets: slot 0 = 0x2E/0x2F,
    /// slot 1 = 0x4E/0x4F.
    pub fn select_slot(&self, slot: u64) -> Result<()> {
        self.call_action(c"ioctl_select_slot", &[slot])
    }

    /// Scan for the SuperIO I/O decode windows (BARs) and add them to the module's
    /// port-access allow-list. Must be called (after `select_slot`) before raw
    /// `pio_inb`/`pio_outb` to a hardware-monitor base port will be permitted.
    pub fn find_bars(&self) -> Result<()> {
        self.call_action(c"ioctl_find_bars", &[])
    }
}

impl Drop for PawnIoModule {
    fn drop(&mut self) {
        unsafe { (self.lib.close)(self.handle) };
    }
}
