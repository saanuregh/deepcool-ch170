//! CPU fan RPM from the Nuvoton NCT6701D SuperIO chip via PawnIO port I/O.
//! Access sequence and register map mirror LibreHardwareMonitor's `Nct677X` /
//! `LpcPort`.

use crate::pawnio::PawnIoModule;
use anyhow::Result;
use log::{debug, warn};

const SIO_REGISTER_PORT_2E: u16 = 0x2E; // slot 0 register port
const SIO_ENTER_KEY: u8 = 0x87;
const SIO_EXIT_KEY: u8 = 0xAA;
const SIO_LDN_REGISTER: u8 = 0x07;
const SIO_HWM_LDN: u8 = 0x0B; // hardware-monitor logical device
const SIO_BASE_ADDRESS_REGISTER: u8 = 0x60;
const SIO_IO_SPACE_LOCK_REGISTER: u8 = 0x28;

// NCT6701D hardware-monitor register access (bank-switched, relative to base port).
const NCT_ADDRESS_OFFSET: u16 = 0x05;
const NCT_DATA_OFFSET: u16 = 0x06;
const NCT_BANK_SELECT_REGISTER: u8 = 0x4E;
// fan/1 = "CPU Fan"; _fanCountRegister[1] in LHM's modern-Nuvoton group.
const NCT_CPU_FAN_COUNT_REGISTER: u16 = 0x4B2;
const NCT_MIN_FAN_COUNT: i32 = 0x15;

pub struct FanSensor {
    lpc: Option<PawnIoModule>,
    base: Option<u16>, // NCT6701D hardware-monitor I/O base port
}

impl FanSensor {
    pub fn new(lpc: Option<PawnIoModule>) -> Self {
        // One-shot detection of the hardware-monitor base port.
        let base = lpc.as_ref().and_then(|lpc| match detect_fan_base(lpc) {
            Ok(base) => {
                debug!("NCT6701D HWM base detected: 0x{base:04x}");
                Some(base)
            }
            Err(err) => {
                warn!("fan base detection failed; fan RPM disabled: {err:?}");
                None
            }
        });
        Self { lpc, base }
    }

    /// CPU fan RPM (fan/1 "CPU Fan"), or 0 if the chip is unavailable. On a
    /// transient read error the caller keeps the previous reading.
    pub fn rpm(&self) -> Result<f64> {
        let (Some(lpc), Some(base)) = (self.lpc.as_ref(), self.base) else {
            return Ok(0.0);
        };
        // 13-bit pulse count across two consecutive registers.
        let high = nct_read_byte(lpc, base, NCT_CPU_FAN_COUNT_REGISTER)? as i32;
        let low = nct_read_byte(lpc, base, NCT_CPU_FAN_COUNT_REGISTER + 1)? as i32;
        let count = (high << 5) | (low & 0x1F);
        if count >= NCT_MIN_FAN_COUNT {
            Ok(1_350_000.0 / count as f64)
        } else {
            Ok(0.0) // stopped / below measurable threshold
        }
    }
}

/// Enter SuperIO config on slot 0 (0x2E), populate the port allow-list, read the
/// hardware-monitor I/O base port, disable the Nuvoton I/O-space lock, and exit.
/// Done once; the fan reads afterward are plain port I/O.
fn detect_fan_base(lpc: &PawnIoModule) -> Result<u16> {
    lpc.select_slot(0)?;
    lpc.pio_outb(SIO_REGISTER_PORT_2E, SIO_ENTER_KEY)?;
    lpc.pio_outb(SIO_REGISTER_PORT_2E, SIO_ENTER_KEY)?;

    // Populate the module's port allow-list (needs config mode entered) so pio
    // reads to the HWM base succeed.
    lpc.find_bars()?;

    lpc.superio_outb(SIO_LDN_REGISTER, SIO_HWM_LDN)?;
    let mut base = lpc.superio_inw(SIO_BASE_ADDRESS_REGISTER)?;
    // Some Fintek chips add a +5; mask it off (matches LHM).
    if (base & 0x07) == 0x05 {
        base &= 0xFFF8;
    }

    // Unlock the hardware-monitor I/O space if locked (NCT679xD).
    let lock = lpc.superio_inb(SIO_IO_SPACE_LOCK_REGISTER)?;
    if lock & 0x10 != 0 {
        lpc.superio_outb(SIO_IO_SPACE_LOCK_REGISTER, lock & !0x10)?;
    }

    lpc.pio_outb(SIO_REGISTER_PORT_2E, SIO_EXIT_KEY)?;

    if base == 0 || base == 0xFFFF {
        anyhow::bail!("implausible HWM base port 0x{base:04x}");
    }
    Ok(base)
}

/// Read one NCT6701D hardware-monitor register (16-bit address, bank-switched).
fn nct_read_byte(lpc: &PawnIoModule, base: u16, address: u16) -> Result<u8> {
    let bank = (address >> 8) as u8;
    let register = (address & 0xFF) as u8;
    lpc.pio_outb(base + NCT_ADDRESS_OFFSET, NCT_BANK_SELECT_REGISTER)?;
    lpc.pio_outb(base + NCT_DATA_OFFSET, bank)?;
    lpc.pio_outb(base + NCT_ADDRESS_OFFSET, register)?;
    lpc.pio_inb(base + NCT_DATA_OFFSET)
}
