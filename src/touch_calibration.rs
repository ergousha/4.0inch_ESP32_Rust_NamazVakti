//! Touchscreen calibration: NVS persistence, the first-boot 5-point wizard, and
//! the hold-to-recalibrate startup gesture (issue #7).
//!
//! The affine solve and the [`Calibration`] type itself are pure math and live
//! in the `namaz-vakti-logic` crate (`logic/src/touch_calibration.rs`) so they
//! can be unit tested on a host toolchain; this module is the hardware-facing
//! half — it drives the panel, samples the touch controller, and stores the
//! result. NVS access mirrors the pattern in [`crate::cache`].

use std::thread::sleep;
use std::time::{Duration, Instant};

use embedded_graphics::{
    mono_font::{
        iso_8859_9::{FONT_9X15, FONT_10X20},
        MonoTextStyle,
    },
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle},
    text::{Alignment, Text},
};
use embedded_hal::spi::SpiDevice;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use crate::touch::Xpt2046;
use crate::Rgb565;

pub use namaz_vakti_logic::touch_calibration::Calibration;
use namaz_vakti_logic::touch_calibration::{solve_affine, vendor_default, CalPoint};

const NAMESPACE: &str = "touch";
const KEY: &str = "calib";

/// Corner targets are inset this far (percent of each axis) from the true edges
/// — resistive panels are least linear right at the border, so tapping dead in
/// the corner gives the worst samples.
const INSET_PERCENT: i32 = 12;

/// Max acceptable error, in logical pixels, between the center target and where
/// the freshly-solved transform predicts the (independent) center tap lands.
const VERIFY_TOLERANCE_PX: f64 = 15.0;

/// How many full wizard passes to attempt before giving up on a clean solve and
/// falling back to the vendor defaults.
const MAX_ATTEMPTS: u32 = 3;

/// Per-target timeout: if no tap lands on a target within this long, the panel
/// is assumed unusable (no stylus, dead digitizer) and the wizard bails to the
/// vendor-default fallback rather than hanging forever.
const POINT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the screen must be held during the boot splash to trigger a
/// re-calibration (clears saved data and re-runs the wizard).
const RECAL_HOLD_SECS: u64 = 5;

/// Outcome of [`run_wizard`]: a cleanly solved-and-verified calibration worth
/// persisting, or the vendor-default fallback which is used for the session but
/// intentionally *not* saved (so the next boot tries the wizard again).
pub enum CalibrationOutcome {
    Calibrated(Calibration),
    FellBack(Calibration),
}

impl CalibrationOutcome {
    pub fn calibration(&self) -> Calibration {
        match self {
            Self::Calibrated(c) | Self::FellBack(c) => *c,
        }
    }

    pub fn should_persist(&self) -> bool {
        matches!(self, Self::Calibrated(_))
    }
}

/// Opens (creating if needed) the `touch` NVS namespace. Mirrors
/// [`crate::cache::open`].
pub fn open(nvs: EspDefaultNvsPartition) -> anyhow::Result<EspNvs<NvsDefault>> {
    Ok(EspNvs::new(nvs, NAMESPACE, true)?)
}

/// Loads a saved calibration, or `None` if none is stored / it's unreadable.
///
/// Stored as a fixed 24-byte little-endian blob rather than JSON — see the note
/// on [`Calibration`] about the Xtensa `serde_json`-float codegen bug.
pub fn load(nvs: &EspNvs<NvsDefault>) -> Option<Calibration> {
    let len = match nvs.blob_len(KEY) {
        Ok(Some(len)) => len,
        _ => return None,
    };
    let mut buf = vec![0u8; len];
    match nvs.get_blob(KEY, &mut buf) {
        Ok(Some(bytes)) => Calibration::from_bytes(bytes),
        _ => None,
    }
}

pub fn save(nvs: &EspNvs<NvsDefault>, cal: &Calibration) {
    if let Err(e) = nvs.set_blob(KEY, &cal.to_bytes()) {
        log::warn!("Failed to persist touch calibration to NVS: {e:?}");
    }
}

/// Removes any saved calibration so the next `load` misses and the wizard runs.
pub fn clear(nvs: &EspNvs<NvsDefault>) {
    if let Err(e) = nvs.remove(KEY) {
        log::warn!("Failed to clear saved touch calibration: {e:?}");
    }
}

/// Detects the "hold the screen through the boot splash to re-calibrate"
/// gesture. Returns `true` only if the panel is held continuously for
/// [`RECAL_HOLD_SECS`]. Returns immediately (no delay) on a normal boot where
/// the screen isn't being touched as this is called.
pub fn recalibration_requested<D, SPI>(display: &mut D, touch: &mut Xpt2046<SPI>) -> bool
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    // Only engage if the panel is already under pressure right now.
    if !matches!(touch.is_touched(), Ok(true)) {
        return false;
    }

    let start = Instant::now();
    let hold = Duration::from_secs(RECAL_HOLD_SECS);
    let mut shown = u64::MAX;
    while start.elapsed() < hold {
        if !matches!(touch.is_touched(), Ok(true)) {
            return false; // released too early — not the gesture
        }
        let remaining = hold.saturating_sub(start.elapsed()).as_secs() + 1;
        if remaining != shown {
            shown = remaining;
            let msg = format!("Keep holding... {remaining}");
            let _ = crate::draw_status(display, &["Recalibrating touch", msg.as_str()]);
        }
        sleep(Duration::from_millis(50));
    }
    true
}

/// Runs the 5-point calibration wizard to completion, returning either a
/// verified calibration or the vendor-default fallback (see [`CalibrationOutcome`]).
pub fn run_wizard<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    width: u16,
    height: u16,
) -> CalibrationOutcome
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    let inset_x = width as i32 * INSET_PERCENT / 100;
    let inset_y = height as i32 * INSET_PERCENT / 100;
    let corners = [
        Point::new(inset_x, inset_y),
        Point::new(width as i32 - 1 - inset_x, inset_y),
        Point::new(inset_x, height as i32 - 1 - inset_y),
        Point::new(width as i32 - 1 - inset_x, height as i32 - 1 - inset_y),
    ];
    let center = Point::new(width as i32 / 2, height as i32 / 2);

    for attempt in 1..=MAX_ATTEMPTS {
        // Capture the 4 corners.
        let mut raws = [(0u16, 0u16); 4];
        let mut timed_out = false;
        for (i, corner) in corners.iter().enumerate() {
            match capture_point(display, touch, *corner, i + 1, width, height) {
                Some(raw) => raws[i] = raw,
                None => {
                    timed_out = true;
                    break;
                }
            }
        }
        // A no-touch timeout won't fix itself on a retry — go straight to the
        // fallback instead of burning three 30s+ passes.
        if timed_out {
            break;
        }

        // Capture the center as an independent verification tap.
        let Some(center_raw) = capture_point(display, touch, center, 5, width, height) else {
            break;
        };

        let points: Vec<CalPoint> = corners
            .iter()
            .zip(raws.iter())
            .map(|(target, raw)| CalPoint {
                raw_x: raw.0 as f64,
                raw_y: raw.1 as f64,
                screen_x: target.x as f64,
                screen_y: target.y as f64,
            })
            .collect();

        let Some(cal) = solve_affine(&points) else {
            log::warn!("Calibration solve failed (degenerate taps), attempt {attempt}/{MAX_ATTEMPTS}");
            let _ = crate::draw_status(display, &["Calibration failed", "Please retry"]);
            sleep(Duration::from_secs(2));
            continue;
        };

        let (px, py) = cal.predict(center_raw.0 as f64, center_raw.1 as f64);
        let err = ((px - center.x as f64).powi(2) + (py - center.y as f64).powi(2)).sqrt();
        if err <= VERIFY_TOLERANCE_PX {
            log::info!("Touch calibration verified (center error {err:.1}px): {cal:?}");
            let _ = crate::draw_status(display, &["Calibration complete"]);
            sleep(Duration::from_millis(800));
            return CalibrationOutcome::Calibrated(cal);
        }

        log::warn!(
            "Calibration verify failed (center error {err:.1}px > {VERIFY_TOLERANCE_PX}px), attempt {attempt}/{MAX_ATTEMPTS}"
        );
        let _ = crate::draw_status(display, &["Calibration failed", "Please retry"]);
        sleep(Duration::from_secs(2));
    }

    log::warn!("Touch calibration falling back to vendor defaults (not persisted)");
    let _ = crate::draw_status(display, &["Calibration skipped", "Using default values"]);
    sleep(Duration::from_secs(2));
    CalibrationOutcome::FellBack(vendor_default(width, height))
}

/// Draws one target, waits for a stable averaged tap on it (touch-down →
/// sample → release), and returns the raw ADC reading. `None` on timeout.
fn capture_point<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    target: Point,
    step: usize,
    width: u16,
    height: u16,
) -> Option<(u16, u16)>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    draw_target(display, target, step, width, height);

    // Touch-down: wait for a stable averaged sample under continuous pressure.
    let deadline = Instant::now() + POINT_TIMEOUT;
    let sample = loop {
        if Instant::now() >= deadline {
            log::warn!("Calibration point {step}/5 timed out waiting for a tap");
            return None;
        }
        match touch.sample_position() {
            Ok(Some(raw)) => break raw,
            Ok(None) => {}
            Err(e) => log::warn!("Touch read failed during calibration: {e:?}"),
        }
        sleep(Duration::from_millis(20));
    };

    // Acknowledge the capture, then wait for release so one long press can't
    // satisfy two consecutive targets.
    draw_captured(display, target);
    let release_deadline = Instant::now() + POINT_TIMEOUT;
    while matches!(touch.is_touched(), Ok(true)) {
        if Instant::now() >= release_deadline {
            break; // don't hang if pressure never clears
        }
        sleep(Duration::from_millis(20));
    }

    Some(sample)
}

/// Clears the screen and draws the target crosshair plus instructions. The
/// instruction block is placed in the half of the screen opposite the target so
/// text never sits on top of the crosshair.
fn draw_target<D>(display: &mut D, target: Point, step: usize, width: u16, height: u16)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = display.clear(crate::col_bg());

    let text_y = if target.y * 2 < height as i32 {
        height as i32 * 3 / 4
    } else {
        height as i32 / 4
    };
    let cx = width as i32 / 2;

    let _ = Text::with_alignment(
        "Touch Calibration",
        Point::new(cx, text_y - 20),
        MonoTextStyle::new(&FONT_10X20, crate::col_text()),
        Alignment::Center,
    )
    .draw(display);
    let step_line = format!("Point {step} of 5");
    let _ = Text::with_alignment(
        &step_line,
        Point::new(cx, text_y + 6),
        MonoTextStyle::new(&FONT_9X15, crate::col_accent()),
        Alignment::Center,
    )
    .draw(display);
    let _ = Text::with_alignment(
        "Tap the crosshair with a stylus",
        Point::new(cx, text_y + 30),
        MonoTextStyle::new(&FONT_9X15, crate::col_dim()),
        Alignment::Center,
    )
    .draw(display);

    draw_crosshair(display, target, crate::col_accent());
}

/// Draws the target crosshair: a ring with a cross through its center.
fn draw_crosshair<D>(display: &mut D, p: Point, color: Rgb565)
where
    D: DrawTarget<Color = Rgb565>,
{
    const ARM: i32 = 12;
    let stroke = PrimitiveStyle::with_stroke(color, 2);
    let _ = Line::new(Point::new(p.x - ARM, p.y), Point::new(p.x + ARM, p.y))
        .into_styled(stroke)
        .draw(display);
    let _ = Line::new(Point::new(p.x, p.y - ARM), Point::new(p.x, p.y + ARM))
        .into_styled(stroke)
        .draw(display);
    let _ = Circle::with_center(p, (ARM as u32) * 2)
        .into_styled(stroke)
        .draw(display);
}

/// Visual acknowledgement drawn once a target's tap is captured.
fn draw_captured<D>(display: &mut D, p: Point)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Circle::with_center(p, 8)
        .into_styled(PrimitiveStyle::with_fill(crate::col_text()))
        .draw(display);
}
