//! Fetches and models prayer times from the ezanvakti.emushaf.net API for
//! Haarlem, Netherlands (Ulke=HOLLANDA id 4, Ilce=HAARLEM id 13877).

use embedded_svc::http::{client::Client as HttpClient, Method};
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use serde::{Deserialize, Serialize};

use crate::time_utils::parse_hhmm_to_seconds;

/// Ilce (district) id for Haarlem under Ulke (country) HOLLANDA.
const ILCE_ID: u32 = 13877;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTimes {
    #[serde(rename = "MiladiTarihKisa")]
    pub date: String,
    /// Hijri date, e.g. "17.1.1448" (day.month.year) — shown when the
    /// dashboard's date mode is toggled by tapping the screen.
    #[serde(rename = "HicriTarihKisa")]
    pub hijri_date: String,
    #[serde(rename = "Imsak")]
    pub imsak: String,
    #[serde(rename = "Ogle")]
    pub ogle: String,
    #[serde(rename = "Ikindi")]
    pub ikindi: String,
    #[serde(rename = "Aksam")]
    pub aksam: String,
    #[serde(rename = "Yatsi")]
    pub yatsi: String,
}

/// The 5 daily prayers in order, as (label, "HH:MM") pairs. Sunrise ("Gunes")
/// is deliberately excluded since it isn't one of the 5 prayers.
impl DayTimes {
    pub fn prayers(&self) -> [(&'static str, &str); 5] {
        [
            ("İMSAK", &self.imsak),
            ("ÖĞLE", &self.ogle),
            ("İKİNDİ", &self.ikindi),
            ("AKŞAM", &self.aksam),
            ("YATSI", &self.yatsi),
        ]
    }

    pub fn prayer_seconds(&self) -> [(&'static str, u32); 5] {
        self.prayers()
            .map(|(name, hhmm)| (name, parse_hhmm_to_seconds(hhmm).unwrap_or(0)))
    }
}

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
