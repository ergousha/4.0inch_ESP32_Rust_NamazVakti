//! Hardware-facing half of the location feature (issue #21): the shared HTTPS
//! GET helper, the three Ezan Vakti list-endpoint fetches, and the NVS
//! persistence of the user's confirmed [`SelectedLocation`].
//!
//! The models, JSON parsing, typed-search filtering, and the persisted-record
//! serialization are pure, host-testable code in the `namaz-vakti-logic` crate
//! (`logic/src/location.rs`); this module re-exports them and adds only the
//! ESP-IDF-dependent I/O. Follows the open/load/save NVS convention of
//! [`crate::cache`] and [`crate::wifi_credentials`], storing the selection as a
//! `serde_json` blob (strings only — safe through the Xtensa codegen).

use embedded_svc::http::{client::Client as HttpClient, Method};
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

pub use namaz_vakti_logic::location::{filter, LocationEntry, SelectedLocation};

const NAMESPACE: &str = "location";
const KEY: &str = "selected";

// --- HTTP ---

/// Performs an HTTPS GET and returns the full response body. Shared by the
/// prayer-time fetch and the three location list fetches so the TLS/bundle
/// config and chunked-read loop live in exactly one place.
pub fn http_get(url: &str) -> anyhow::Result<Vec<u8>> {
    let http_config = HttpConfig {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(std::time::Duration::from_secs(20)),
        ..Default::default()
    };
    let mut client = HttpClient::wrap(EspHttpConnection::new(&http_config)?);

    let request = client.request(Method::Get, url, &[("accept", "application/json")])?;
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
    Ok(body)
}

/// Fetches and parses the list of countries (`GET /ulkeler`).
pub fn fetch_countries() -> anyhow::Result<Vec<LocationEntry>> {
    let body = http_get("https://ezanvakti.emushaf.net/ulkeler")?;
    Ok(namaz_vakti_logic::location::parse_entries(&body)?)
}

/// Fetches and parses the cities of a country (`GET /sehirler/{country_id}`).
pub fn fetch_cities(country_id: &str) -> anyhow::Result<Vec<LocationEntry>> {
    let url = format!("https://ezanvakti.emushaf.net/sehirler/{country_id}");
    let body = http_get(&url)?;
    Ok(namaz_vakti_logic::location::parse_entries(&body)?)
}

/// Fetches and parses the districts of a city (`GET /ilceler/{city_id}`).
pub fn fetch_districts(city_id: &str) -> anyhow::Result<Vec<LocationEntry>> {
    let url = format!("https://ezanvakti.emushaf.net/ilceler/{city_id}");
    let body = http_get(&url)?;
    Ok(namaz_vakti_logic::location::parse_entries(&body)?)
}

// --- NVS persistence ---

/// Opens (creating if needed) the `location` NVS namespace. Mirrors
/// [`crate::cache::open`].
pub fn open(nvs: EspDefaultNvsPartition) -> anyhow::Result<EspNvs<NvsDefault>> {
    Ok(EspNvs::new(nvs, NAMESPACE, true)?)
}

/// Loads the saved selection, falling back to [`SelectedLocation::default`]
/// (Haarlem) when nothing is stored, the blob is unreadable/corrupt, or the
/// stored record has no district id — so the device always has a usable
/// `/vakitler` key and behaves exactly as the pre-#21 firmware did on a fresh
/// device.
pub fn load(nvs: &EspNvs<NvsDefault>) -> SelectedLocation {
    let len = match nvs.blob_len(KEY) {
        Ok(Some(len)) => len,
        _ => return SelectedLocation::default(),
    };
    let mut buf = vec![0u8; len];
    let selected: Option<SelectedLocation> = match nvs.get_blob(KEY, &mut buf) {
        Ok(Some(bytes)) => serde_json::from_slice(bytes).ok(),
        _ => None,
    };
    match selected {
        Some(sel) if sel.is_valid() => sel,
        _ => SelectedLocation::default(),
    }
}

/// Persists the given selection. Logs and swallows write errors (consistent
/// with the other NVS modules — a failed persist must not crash the device).
pub fn save(nvs: &EspNvs<NvsDefault>, selected: &SelectedLocation) {
    match serde_json::to_vec(selected) {
        Ok(bytes) => {
            if let Err(e) = nvs.set_blob(KEY, &bytes) {
                log::warn!("Failed to persist location to NVS: {e:?}");
            }
        }
        Err(e) => log::warn!("Failed to serialize location: {e:?}"),
    }
}
