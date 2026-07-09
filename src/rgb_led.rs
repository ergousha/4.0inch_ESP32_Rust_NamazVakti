//! Driver for the board's onboard **RGB tricolor status LED** (three discrete
//! R/G/B LEDs, each on its own GPIO — see issue #17).
//!
//! The LED is wired **common-anode / active-low**: driving a pin *low* turns
//! its channel on, *high* turns it off. That inversion is hidden entirely
//! inside this driver — callers think purely in terms of "on = lit" via the
//! semantic [`Zone`] API and never touch a GPIO or a raw level.
//!
//! ```text
//! LED    GPIO   on = pin low (common anode)
//! Red    IO22
//! Green  IO16
//! Blue   IO17
//! ```
//!
//! The color follows the dashboard's fıkh zone (Fazilet → green, Cevaz → amber,
//! Kerahet → red); the zone→pattern mapping is the pure, host-tested
//! [`namaz_vakti_logic::zone`] logic so the LED and the on-screen status bar can
//! never drift apart. Writes are gated on change, so pushing the same zone every
//! second is a cheap no-op rather than three redundant GPIO writes.

use esp_idf_svc::hal::gpio::{Output, OutputPin, PinDriver};
use esp_idf_svc::sys::EspError;

use namaz_vakti_logic::zone::{LedColor, Zone};

/// Owns the three output pins of the common-anode RGB LED and tracks the last
/// zone pushed so redundant writes are skipped.
pub struct RgbLed<'d> {
    red: PinDriver<'d, Output>,
    green: PinDriver<'d, Output>,
    blue: PinDriver<'d, Output>,
    /// The last state written. `None` means "nothing written yet"; the inner
    /// `Option<Zone>` mirrors [`set_zone`](RgbLed::set_zone)'s argument (an inner
    /// `None` is the LED-off state).
    last: Option<Option<Zone>>,
}

impl<'d> RgbLed<'d> {
    /// Takes the three LED pins (GPIO 22 red, 16 green, 17 blue) and configures
    /// them as outputs, starting with the LED **off** (all channels high, since
    /// the wiring is active-low).
    pub fn new(
        red: impl OutputPin + 'd,
        green: impl OutputPin + 'd,
        blue: impl OutputPin + 'd,
    ) -> Result<Self, EspError> {
        let mut led = RgbLed {
            red: PinDriver::output(red)?,
            green: PinDriver::output(green)?,
            blue: PinDriver::output(blue)?,
            last: None,
        };
        // Start dark and prime `last` so the first real `set_zone` still writes.
        led.write(LedColor::OFF)?;
        Ok(led)
    }

    /// Sets the LED to the color of the given fıkh zone, or turns it off when
    /// there is no active zone (`None` — before the day's first entry or when
    /// prayer data is missing).
    ///
    /// Only issues GPIO writes when the zone actually differs from the last one
    /// pushed, mirroring the dashboard's current-box redraw gating.
    pub fn set_zone(&mut self, zone: Option<Zone>) -> Result<(), EspError> {
        if self.last == Some(zone) {
            return Ok(());
        }
        let color = zone.map(Zone::led_color).unwrap_or(LedColor::OFF);
        self.write(color)?;
        self.last = Some(zone);
        Ok(())
    }

    /// Drives all three pins to the given pattern, applying the active-low
    /// common-anode inversion (on → low, off → high). Does *not* touch `last`.
    fn write(&mut self, color: LedColor) -> Result<(), EspError> {
        write_channel(&mut self.red, color.red)?;
        write_channel(&mut self.green, color.green)?;
        write_channel(&mut self.blue, color.blue)?;
        Ok(())
    }
}

/// Writes one channel: `on` lights the LED, which on this common-anode wiring
/// means pulling the pin **low**.
fn write_channel(pin: &mut PinDriver<'_, Output>, on: bool) -> Result<(), EspError> {
    if on {
        pin.set_low()
    } else {
        pin.set_high()
    }
}
