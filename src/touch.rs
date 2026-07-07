//! Minimal XPT2046 resistive touch driver.
//!
//! Reads the Z1/Z2 pressure channels for "is the screen being touched?" (used
//! by the main loop's tap-to-toggle-date-mode gesture) and the X/Y position
//! channels for raw coordinates (used by the touch-calibration wizard and,
//! once calibrated, future touch-driven UI — see `touch_calibration`).

use embedded_hal::spi::SpiDevice;

const CMD_READ_Z1: u8 = 0xB0;
const CMD_READ_Z2: u8 = 0xC0;
const CMD_READ_X: u8 = 0xD0;
const CMD_READ_Y: u8 = 0x90;

/// Raw position samples taken per [`Xpt2046::sample_position`] call. The first
/// [`DISCARD_SAMPLES`] are thrown away (resistive panels are noisy for the
/// first moment after contact); the rest are averaged.
const POSITION_SAMPLES: usize = 8;
const DISCARD_SAMPLES: usize = 2;

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

    /// Reads a single raw 12-bit `(x, y)` ADC pair. The values are only
    /// meaningful while the panel is actually being pressed — callers that need
    /// a trustworthy coordinate should use [`Self::sample_position`] instead.
    pub fn read_position(&mut self) -> Result<(u16, u16), SPI::Error> {
        let x = self.read_channel(CMD_READ_X)?;
        let y = self.read_channel(CMD_READ_Y)?;
        Ok((x, y))
    }

    /// Returns `Some((x_raw, y_raw))` — an average of several raw readings taken
    /// while the panel stays under pressure — or `None` if it isn't currently
    /// touched (or is released mid-sample). The first [`DISCARD_SAMPLES`]
    /// readings are discarded before averaging to shed the initial-contact
    /// noise typical of resistive panels.
    pub fn sample_position(&mut self) -> Result<Option<(u16, u16)>, SPI::Error> {
        if !self.is_touched()? {
            return Ok(None);
        }
        let mut sum_x: u32 = 0;
        let mut sum_y: u32 = 0;
        let mut kept: u32 = 0;
        for i in 0..POSITION_SAMPLES {
            // Bail if the touch is released partway through: a partial average
            // straddling the release is worse than reporting "no touch".
            if !self.is_touched()? {
                return Ok(None);
            }
            let (x, y) = self.read_position()?;
            if i >= DISCARD_SAMPLES {
                sum_x += x as u32;
                sum_y += y as u32;
                kept += 1;
            }
        }
        if kept == 0 {
            return Ok(None);
        }
        Ok(Some(((sum_x / kept) as u16, (sum_y / kept) as u16)))
    }
}
