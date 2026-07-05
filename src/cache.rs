//! Persists the fetched prayer-time month to NVS flash, so a reboot can show
//! the dashboard immediately instead of blocking on a fresh HTTPS fetch.

use esp_idf_svc::nvs::{EspNvs, NvsDefault};

use crate::prayer::DayTimes;

const NAMESPACE: &str = "namaz";
const KEY: &str = "days";

/// Loads the cached month, if any. Returns an empty `Vec` (never an error)
/// on a missing/corrupt cache — the caller falls back to a fresh fetch.
pub fn load(nvs: &EspNvs<NvsDefault>) -> Vec<DayTimes> {
    let len = match nvs.blob_len(KEY) {
        Ok(Some(len)) => len,
        _ => return Vec::new(),
    };
    let mut buf = vec![0u8; len];
    match nvs.get_blob(KEY, &mut buf) {
        Ok(Some(bytes)) => serde_json::from_slice(bytes).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn save(nvs: &EspNvs<NvsDefault>, days: &[DayTimes]) {
    match serde_json::to_vec(days) {
        Ok(bytes) => {
            if let Err(e) = nvs.set_blob(KEY, &bytes) {
                log::warn!("Failed to cache prayer data to NVS: {e:?}");
            }
        }
        Err(e) => log::warn!("Failed to serialize prayer data for caching: {e:?}"),
    }
}

pub fn open(nvs: esp_idf_svc::nvs::EspDefaultNvsPartition) -> anyhow::Result<EspNvs<NvsDefault>> {
    Ok(EspNvs::new(nvs, NAMESPACE, true)?)
}
