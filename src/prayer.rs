//! Fetches prayer times from the ezanvakti.emushaf.net API for the selected
//! district. The district id comes from the persisted [`crate::location`]
//! selection (defaulting to Haarlem, Ilce id 13877, the firmware's historical
//! location); it is no longer hard-coded here.
//!
//! The `DayTimes` model and its pure parsing/formatting helpers live in the
//! `namaz-vakti-logic` crate (see `logic/src/prayer_times.rs`) so they can be
//! unit tested with a plain host Rust toolchain, with no ESP-IDF/hardware
//! dependency required. Only the actual HTTPS fetch below needs ESP-IDF.

use crate::location::http_get;

pub use namaz_vakti_logic::prayer_times::DayTimes;

/// Downloads the full (~32 day) prayer time table for `district_id` over HTTPS.
pub fn fetch_month(district_id: &str) -> anyhow::Result<Vec<DayTimes>> {
    let url = format!("https://ezanvakti.emushaf.net/vakitler/{district_id}");
    let body = http_get(&url)?;
    let days: Vec<DayTimes> = serde_json::from_slice(&body)?;
    Ok(days)
}
