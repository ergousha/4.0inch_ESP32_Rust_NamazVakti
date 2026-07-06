//! Fetches prayer times from the ezanvakti.emushaf.net API for Haarlem,
//! Netherlands (Ulke=HOLLANDA id 4, Ilce=HAARLEM id 13877).
//!
//! The `DayTimes` model and its pure parsing/formatting helpers live in the
//! `namaz-vakti-logic` crate (see `logic/src/prayer_times.rs`) so they can be
//! unit tested with a plain host Rust toolchain, with no ESP-IDF/hardware
//! dependency required. Only the actual HTTPS fetch below needs ESP-IDF.

use embedded_svc::http::{client::Client as HttpClient, Method};
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

pub use namaz_vakti_logic::prayer_times::DayTimes;

/// Ilce (district) id for Haarlem under Ulke (country) HOLLANDA.
const ILCE_ID: u32 = 13877;

/// Downloads the full (~32 day) prayer time table over HTTPS.
pub fn fetch_month() -> anyhow::Result<Vec<DayTimes>> {
    let http_config = HttpConfig {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(std::time::Duration::from_secs(20)),
        ..Default::default()
    };
    let mut client = HttpClient::wrap(EspHttpConnection::new(&http_config)?);

    let url = format!("https://ezanvakti.emushaf.net/vakitler/{ILCE_ID}");
    let request = client.request(Method::Get, &url, &[("accept", "application/json")])?;
    let mut response = request.submit()?;

    let status = response.status();
    if !(200..300).contains(&status) {
        anyhow::bail!("ezanvakti.emushaf.net returned HTTP {status}");
    }

    let mut body = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = response.read(&mut buf).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }

    let days: Vec<DayTimes> = serde_json::from_slice(&body)?;
    Ok(days)
}
