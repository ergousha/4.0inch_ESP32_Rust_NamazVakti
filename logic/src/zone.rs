//! The fıkh zone of the running prayer-time countdown — the *single source of
//! truth* for the 1/3–2/3 thresholds shared by the on-screen status bar and the
//! onboard RGB status LED.
//!
//! The countdown between two timeline entries is split into three equal thirds:
//! *Fazilet* (the meritorious early time), *Cevaz* (permissible), and *Kerahet*
//! (disliked, running out). This module maps an elapsed fraction to a [`Zone`],
//! and each zone to the LED channel pattern that mirrors the screen's zone
//! color. Keeping this pure (no ESP-IDF / hardware deps) lets it be unit-tested
//! on the host — see the tests at the bottom of this file.
//!
//! The display `Rgb565` color for a zone lives in the firmware crate (it needs
//! `embedded-graphics`); this crate only owns the thresholds and the LED
//! pattern so both consumers agree on which zone is active.

/// Binary on/off state of the onboard common-anode RGB LED's three channels.
///
/// Each channel is a single GPIO with no PWM, so only the 7 primary color
/// mixes (plus off) are reachable — see [`Zone::led_color`] for why *Cevaz* is
/// approximated rather than shown as true orange.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LedColor {
    pub red: bool,
    pub green: bool,
    pub blue: bool,
}

impl LedColor {
    /// All channels dark — used when there is no active zone.
    pub const OFF: LedColor = LedColor {
        red: false,
        green: false,
        blue: false,
    };
}

/// One of the three static fıkh zones of a prayer-time countdown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    /// 0–33% elapsed — the meritorious early time (screen: emerald green).
    Fazilet,
    /// 33–66% elapsed — permissible (screen: warm orange).
    Cevaz,
    /// 66–100% elapsed — disliked, time running out (screen: warning red).
    Kerahet,
}

impl Zone {
    /// The zone for a given elapsed fraction of the current interval. This is
    /// the *only* place the 1/3 and 2/3 thresholds are defined; both the status
    /// bar / current-vakit tint and the RGB LED derive their state from here so
    /// they can never drift apart. `progress` is clamped to `0.0..=1.0`.
    pub fn from_progress(progress: f32) -> Zone {
        let p = progress.clamp(0.0, 1.0);
        if p < 1.0 / 3.0 {
            Zone::Fazilet
        } else if p < 2.0 / 3.0 {
            Zone::Cevaz
        } else {
            Zone::Kerahet
        }
    }

    /// The RGB LED channel pattern that mirrors this zone's status-bar color.
    ///
    /// Because each channel is binary on/off (one GPIO, no PWM), the panel's
    /// warm orange for *Cevaz* is not reachable. It is deliberately approximated
    /// as **Red + Green (amber/yellow)** — the closest primary mix — so the LED
    /// still reads as a distinct "middle" state between green and red.
    pub fn led_color(self) -> LedColor {
        match self {
            // Fazilet → Emerald Green: green channel only.
            Zone::Fazilet => LedColor {
                red: false,
                green: true,
                blue: false,
            },
            // Cevaz → Warm Orange, approximated as amber (Red + Green). True
            // orange would need per-channel PWM brightness (see issue #17).
            Zone::Cevaz => LedColor {
                red: true,
                green: true,
                blue: false,
            },
            // Kerahet → Warning Red: red channel only.
            Zone::Kerahet => LedColor {
                red: true,
                green: false,
                blue: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_split_into_thirds() {
        assert_eq!(Zone::from_progress(0.0), Zone::Fazilet);
        assert_eq!(Zone::from_progress(0.32), Zone::Fazilet);
        // Boundary at 1/3 flips into Cevaz.
        assert_eq!(Zone::from_progress(1.0 / 3.0), Zone::Cevaz);
        assert_eq!(Zone::from_progress(0.5), Zone::Cevaz);
        // Boundary at 2/3 flips into Kerahet.
        assert_eq!(Zone::from_progress(2.0 / 3.0), Zone::Kerahet);
        assert_eq!(Zone::from_progress(0.99), Zone::Kerahet);
        assert_eq!(Zone::from_progress(1.0), Zone::Kerahet);
    }

    #[test]
    fn out_of_range_progress_is_clamped() {
        assert_eq!(Zone::from_progress(-5.0), Zone::Fazilet);
        assert_eq!(Zone::from_progress(42.0), Zone::Kerahet);
    }

    #[test]
    fn led_pattern_matches_zone_color() {
        // Fazilet → pure green.
        assert_eq!(
            Zone::Fazilet.led_color(),
            LedColor {
                red: false,
                green: true,
                blue: false
            }
        );
        // Cevaz → amber (Red + Green) approximation of orange.
        assert_eq!(
            Zone::Cevaz.led_color(),
            LedColor {
                red: true,
                green: true,
                blue: false
            }
        );
        // Kerahet → pure red.
        assert_eq!(
            Zone::Kerahet.led_color(),
            LedColor {
                red: true,
                green: false,
                blue: false
            }
        );
        // Blue is never lit by any zone (reserved for future state colors).
        for zone in [Zone::Fazilet, Zone::Cevaz, Zone::Kerahet] {
            assert!(!zone.led_color().blue);
        }
    }

    #[test]
    fn off_is_all_dark() {
        assert_eq!(
            LedColor::OFF,
            LedColor {
                red: false,
                green: false,
                blue: false
            }
        );
    }
}
