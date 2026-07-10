//! On-device location selection UI (issue #21): a typed-search + bounded-picker
//! flow over the Ezan Vakti country → city → district hierarchy.
//!
//! Follows the same interaction model as the WiFi setup flow — a "draw once,
//! then poll `sample_position` in a debounced loop" per screen, the shared
//! on-screen [`crate::keyboard`] for the search box, and the header back-arrow
//! tap square as the cancel/return control. Rather than render the full
//! (potentially thousands-long) API list on a resistive-touch panel, each level
//! fetches its list once, then the user types a query and picks from the capped,
//! filtered matches.
//!
//! The pure list model / parsing / filtering lives in `namaz-vakti-logic`; the
//! HTTPS list fetches and NVS persistence live in [`crate::location`]. This
//! module only orchestrates the UI and returns the confirmed selection; the
//! caller ([`crate::main`]) persists it and refreshes the prayer-time cache.

use std::thread::sleep;
use std::time::Duration;

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyleBuilder, Rectangle},
};
use embedded_hal::spi::SpiDevice;

use namaz_vakti_logic::language::{self, Language, Msg};

use crate::keyboard;
use crate::location::{self, LocationEntry, SelectedLocation};
use crate::text::{self, HAlign};
use crate::touch::Xpt2046;
use crate::touch_calibration::Calibration;
use crate::{settings_screen, Rgb565};

/// Maximum filtered matches shown at once. Kept small so the resistive-touch
/// rows stay comfortably tappable and the list never scrolls; the user narrows
/// the query instead. A truncation note is shown when more matched.
const MAX_RESULTS: usize = 7;

// --- Results picker geometry (shared by draw + hit-test) ---
const ROW_X: i32 = 8;
const ROW_W: u32 = 464;
const ROW_H: u32 = 34;
const ROW_GAP: i32 = 3;
const LIST_TOP: i32 = 40;

fn row_y(index: usize) -> i32 {
    LIST_TOP + index as i32 * (ROW_H as i32 + ROW_GAP)
}

/// Runs the full country → city → district selection. Returns the confirmed
/// selection, or `None` if the user backed out of the country level (the caller
/// keeps the existing location untouched).
///
/// Implemented as an explicit level state machine rather than recursion so that
/// "go back a level" (repeatable without limit) can never grow the task stack.
pub fn run_setup<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    lang: Language,
) -> anyhow::Result<Option<SelectedLocation>>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    let mut country: Option<LocationEntry> = None;
    let mut city: Option<LocationEntry> = None;

    loop {
        // The current level is the deepest one not yet chosen.
        if country.is_none() {
            match run_level(
                display,
                touch,
                calibration,
                lang,
                Msg::LocationSearchCountry,
                None,
                location::fetch_countries,
            )? {
                Some(c) => country = Some(c),
                None => return Ok(None), // backing out of country cancels the flow
            }
        } else if city.is_none() {
            let c = country.as_ref().unwrap();
            let crumb = c.display_name().to_string();
            match run_level(
                display,
                touch,
                calibration,
                lang,
                Msg::LocationSearchCity,
                Some(&crumb),
                || location::fetch_cities(&c.id),
            )? {
                Some(s) => city = Some(s),
                None => country = None, // back → re-pick the country
            }
        } else {
            let c = country.as_ref().unwrap();
            let s = city.as_ref().unwrap();
            let crumb = format!("{} / {}", c.display_name(), s.display_name());
            match run_level(
                display,
                touch,
                calibration,
                lang,
                Msg::LocationSearchDistrict,
                Some(&crumb),
                || location::fetch_districts(&s.id),
            )? {
                Some(district) => {
                    return Ok(Some(SelectedLocation {
                        country_id: c.id.clone(),
                        country_name: c.display_name().to_string(),
                        city_id: s.id.clone(),
                        city_name: s.display_name().to_string(),
                        district_id: district.id.clone(),
                        district_name: district.display_name().to_string(),
                    }));
                }
                None => city = None, // back → re-pick the city
            }
        }
    }
}

/// The outcome of one level's search+pick loop.
enum LevelResult {
    Picked(LocationEntry),
    Back,
}

/// Runs one level: fetch the list once (with a loading indicator), then loop
/// search-box → filtered picker until the user picks an entry or backs out.
///
/// `fetch` is called once up front; a fetch error shows the error state and
/// returns `Back` (there's nothing to search). `crumb` is the breadcrumb shown
/// alongside the search prompt.
fn run_level<D, SPI, F>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    lang: Language,
    title: Msg,
    crumb: Option<&str>,
    fetch: F,
) -> anyhow::Result<Option<LocationEntry>>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
    F: Fn() -> anyhow::Result<Vec<LocationEntry>>,
{
    crate::draw_status(display, &[language::text(lang, Msg::LocationLoading)], lang)?;
    let entries = match fetch() {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Location list fetch failed: {e:?}");
            crate::draw_status(display, &[language::text(lang, Msg::LocationError)], lang)?;
            sleep(Duration::from_millis(2000));
            return Ok(match_to_option(LevelResult::Back));
        }
    };

    loop {
        let query = match keyboard::run_text_entry(
            display, touch, calibration, lang, title, crumb, MAX_QUERY_LEN,
        )? {
            Some(q) => q,
            None => return Ok(match_to_option(LevelResult::Back)),
        };

        let (matches, truncated) = location::filter(&entries, &query, MAX_RESULTS);
        // Clone the matched entries so the borrow of `entries` doesn't outlive
        // the picker loop below (which mutably borrows the display).
        let matches: Vec<LocationEntry> = matches.into_iter().cloned().collect();

        match run_picker(display, touch, calibration, lang, &matches, truncated)? {
            LevelResult::Picked(entry) => return Ok(Some(entry)),
            LevelResult::Back => continue, // re-search this level
        }
    }
}

/// A search query rarely needs to be long — a few characters narrow any list.
const MAX_QUERY_LEN: usize = 24;

/// `LevelResult::Back` → `None`, `Picked` → `Some`. A tiny helper so the fetch
/// error / cancel paths read clearly.
fn match_to_option(r: LevelResult) -> Option<LocationEntry> {
    match r {
        LevelResult::Picked(e) => Some(e),
        LevelResult::Back => None,
    }
}

/// Draws the filtered matches and polls until the user taps a row (→ `Picked`)
/// or the back arrow (→ `Back`, to re-search this level).
fn run_picker<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    lang: Language,
    matches: &[LocationEntry],
    truncated: bool,
) -> anyhow::Result<LevelResult>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    draw_picker(display, lang, matches, truncated)?;
    keyboard::wait_for_release(touch);

    let mut press_handled = false;
    loop {
        match touch.sample_position() {
            Ok(Some((xr, yr))) => {
                if !press_handled {
                    press_handled = true;
                    let (x, y) = calibration.to_screen(xr, yr);
                    if settings_screen::point_in_icon(x, y) {
                        return Ok(LevelResult::Back);
                    }
                    if let Some(i) = picker_hit(x, y, matches.len()) {
                        return Ok(LevelResult::Picked(matches[i].clone()));
                    }
                }
            }
            Ok(None) => press_handled = false,
            Err(e) => log::warn!("Touch read failed in location picker: {e:?}"),
        }
        sleep(Duration::from_millis(40));
    }
}

/// Maps a calibrated tap to a result row index, if any.
fn picker_hit(x: i32, y: i32, n: usize) -> Option<usize> {
    if x < ROW_X || x >= ROW_X + ROW_W as i32 {
        return None;
    }
    for i in 0..n {
        let ry = row_y(i);
        if y >= ry && y < ry + ROW_H as i32 {
            return Some(i);
        }
    }
    None
}

fn draw_picker<D>(
    display: &mut D,
    lang: Language,
    matches: &[LocationEntry],
    truncated: bool,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(crate::col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    settings_screen::draw_back_icon(display)?;

    if matches.is_empty() {
        // No-results state: the back arrow re-opens the search box.
        text::draw_line(
            display,
            language::text(lang, Msg::LocationNoResults),
            Point::new(240, row_y(1) + 20),
            HAlign::Center,
            crate::col_dim(),
            lang,
            text::Size::Medium,
        )?;
        return Ok(());
    }

    for (i, entry) in matches.iter().enumerate() {
        draw_row(display, i, entry.display_name())?;
    }

    // Note that some matches were dropped by the cap, so an absent row doesn't
    // read as "not in the API".
    if truncated {
        text::draw_line(
            display,
            language::text(lang, Msg::LocationMore),
            Point::new(240, row_y(matches.len()) + 16),
            HAlign::Center,
            crate::col_dim(),
            lang,
            text::Size::Small,
        )?;
    }
    Ok(())
}

/// Draws one full-width result row. Location names come from the API in their
/// own (Latin) script, so they're rendered with the default (Latin) font
/// regardless of the active UI language.
fn draw_row<D>(display: &mut D, index: usize, label: &str) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let y = row_y(index);
    let border = PrimitiveStyleBuilder::new()
        .stroke_color(crate::col_dim())
        .stroke_width(1)
        .fill_color(crate::col_card_bg())
        .build();
    Rectangle::new(Point::new(ROW_X, y), Size::new(ROW_W, ROW_H))
        .into_styled(border)
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    text::draw_line(
        display,
        label,
        Point::new(ROW_X + 12, y + 22),
        HAlign::Left,
        crate::col_text(),
        Language::default(),
        text::Size::Small,
    )?;
    Ok(())
}
