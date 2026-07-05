//! Minimal XPT2046 resistive touch driver — pressure-only, no coordinate
//! reading. This project only needs "was the screen tapped?" to toggle the
//! header's date mode, not X/Y position.

use embedded_hal::spi::SpiDevice;

const CMD_READ_Z1: u8 = 0xB0;
const CMD_READ_Z2: u8 = 0xC0;

/// Empirical pressure threshold. `z1 + (4095 - z2)` reads near 0 when
/// untouched (Z1 low, Z2 near max with the resistive layers not in contact)
/// and rises sharply under finger/stylus pressure — the same heuristic used
/// by the widely-used PJRC/Adafruit `XPT2046_Touchscreen` Arduino driver.
///
/// NOTE: not verified against real hardware (no physical access to the
/// board while writing this) — if taps are missed or fire spuriously,
/// tune this first.
const PRESSURE_THRESHOLD: i32 = 600;

pub struct Xpt2046<SPI> {
    spi: SPI,
}

impl<SPI> Xpt2046<SPI>
where
    SPI: SpiDevice,
{
    pub fn new(spi: SPI) -> Self {
        Self { spi }
    }

    fn read_channel(&mut self, cmd: u8) -> Result<u16, SPI::Error> {
        let mut buf = [cmd, 0, 0];
        self.spi.transfer_in_place(&mut buf)?;
        // The 12-bit conversion result is clocked out MSB-first starting one
        // bit after the command byte, followed by 3 padding bits.
        Ok((((buf[1] as u16) << 8) | buf[2] as u16) >> 3)
    }

    /// Returns `true` if the panel is currently under enough pressure to
    /// count as a touch.
    pub fn is_touched(&mut self) -> Result<bool, SPI::Error> {
        let z1 = self.read_channel(CMD_READ_Z1)? as i32;
        let z2 = self.read_channel(CMD_READ_Z2)? as i32;
        Ok(z1 + (4095 - z2) > PRESSURE_THRESHOLD)
    }
}
