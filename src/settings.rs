//! Persists the user's UI settings (language + header date mode) to NVS flash
//! so they survive reboots. Mirrors the open/load/save pattern in
//! [`crate::cache`] and [`crate::touch_calibration`], but stores each value as
//! a single `u8` rather than JSON — the Xtensa `serde_json`-float codegen bug
//! called out in the touch-calibration module is a non-issue for plain bytes,
//! and two enum discriminants don't need a serializer.

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use namaz_vakti_logic::language::Language;

use crate::DateMode;

const NAMESPACE: &str = "settings";
const KEY_LANG: &str = "lang";
const KEY_DATEMODE: &str = "datemode";

/// The persisted UI settings, loaded once at boot and updated from the settings
/// screen.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub language: Language,
    pub date_mode: DateMode,
}

impl Default for Settings {
    /// Türkçe + Miladi — the firmware's historical defaults, used when nothing
    /// is stored yet.
    fn default() -> Self {
        Settings {
            language: Language::default(),
            date_mode: DateMode::Miladi,
        }
    }
}

/// Opens (creating if needed) the `settings` NVS namespace. Mirrors
/// [`crate::cache::open`].
pub fn open(nvs: EspDefaultNvsPartition) -> anyhow::Result<EspNvs<NvsDefault>> {
    Ok(EspNvs::new(nvs, NAMESPACE, true)?)
}

/// Loads the saved settings, falling back to [`Settings::default`] for any
/// missing/unreadable value.
pub fn load(nvs: &EspNvs<NvsDefault>) -> Settings {
    let defaults = Settings::default();
    Settings {
        language: read_u8(nvs, KEY_LANG)
            .map(Language::from_u8)
            .unwrap_or(defaults.language),
        date_mode: read_u8(nvs, KEY_DATEMODE)
            .map(DateMode::from_u8)
            .unwrap_or(defaults.date_mode),
    }
}

/// Persists the chosen language (single byte).
pub fn save_language(nvs: &EspNvs<NvsDefault>, language: Language) {
    if let Err(e) = nvs.set_u8(KEY_LANG, language.to_u8()) {
        log::warn!("Failed to persist language to NVS: {e:?}");
    }
}

/// Persists the chosen header date mode (single byte).
pub fn save_date_mode(nvs: &EspNvs<NvsDefault>, date_mode: DateMode) {
    if let Err(e) = nvs.set_u8(KEY_DATEMODE, date_mode.to_u8()) {
        log::warn!("Failed to persist date mode to NVS: {e:?}");
    }
}

fn read_u8(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<u8> {
    nvs.get_u8(key).ok().flatten()
}
