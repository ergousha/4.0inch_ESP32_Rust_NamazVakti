//! Pure, host-testable models and helpers for the Ezan Vakti location API
//! hierarchy (country → city → district).
//!
//! Kept hardware-free (like the rest of this crate) so the JSON parsing, the
//! typed-search filtering, and the persisted-selection serialization can be
//! unit tested on a plain host toolchain. The firmware side — the actual HTTPS
//! list fetches and the NVS blob persistence — lives in `src/location.rs` and
//! `src/location_setup.rs`, which re-export the types from here.
//!
//! ## API shape
//!
//! Every level of `GET /ulkeler`, `GET /sehirler/{id}`, `GET /ilceler/{id}`
//! returns a JSON array of objects with the *same* three logical fields under
//! level-specific names:
//!
//! | Level    | name field  | English-name field | id field   |
//! |----------|-------------|---------------------|------------|
//! | country  | `UlkeAdi`   | `UlkeAdiEn`         | `UlkeID`   |
//! | city     | `SehirAdi`  | `SehirAdiEn`        | `SehirID`  |
//! | district | `IlceAdi`   | `IlceAdiEn`         | `IlceID`   |
//!
//! Ids are JSON *strings* (e.g. `"13877"`), so they are kept as strings and
//! treated as opaque canonical identifiers — the code never assumes they are
//! numeric, contiguous, or that the geographic hierarchy is uniform (some
//! countries expose a single city whose districts are the real towns; see the
//! Netherlands in the issue). A [`LocationEntry`] deserializes from any of the
//! three levels via serde field aliases.

use serde::{Deserialize, Serialize};

/// One row from any of the three list endpoints. The multi-alias fields let a
/// single type deserialize a country, a city, or a district row unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LocationEntry {
    #[serde(alias = "UlkeAdi", alias = "SehirAdi", alias = "IlceAdi")]
    pub name: String,
    #[serde(
        alias = "UlkeAdiEn",
        alias = "SehirAdiEn",
        alias = "IlceAdiEn",
        default
    )]
    pub name_en: String,
    #[serde(alias = "UlkeID", alias = "SehirID", alias = "IlceID")]
    pub id: String,
}

impl LocationEntry {
    /// A trimmed display name. The API leaves stray trailing spaces on some
    /// rows (e.g. `"ALBERGEN "`), which would misalign right-aligned labels and
    /// look like typos, so display goes through here.
    pub fn display_name(&self) -> &str {
        self.name.trim()
    }
}

/// Parses a list endpoint body into entries. A malformed body is a hard error
/// (the caller shows the network/API-error state); an empty array parses to an
/// empty `Vec` (the caller shows the no-results state).
pub fn parse_entries(body: &[u8]) -> Result<Vec<LocationEntry>, serde_json::Error> {
    serde_json::from_slice(body)
}

/// Case-insensitive, accent-tolerant substring match used by the typed-search
/// step so the UI never has to render an unbounded list.
///
/// The match is tried against both the localized name and the ASCII English
/// name, because the on-screen keyboard only produces ASCII: typing `AGRI`
/// still finds `AĞRI` (whose `name_en` is `AGRI`). Matching is done on an
/// ASCII-folded, upper-cased form of each so `is` finds `İSTANBUL` etc.
pub fn matches(entry: &LocationEntry, query: &str) -> bool {
    let q = normalize(query);
    if q.is_empty() {
        return true;
    }
    normalize(&entry.name).contains(&q) || normalize(&entry.name_en).contains(&q)
}

/// Filters `entries` to those matching `query`, preserving the API's order and
/// capping the result at `limit` so the picker stays bounded. Returns the
/// matches and whether any were dropped by the cap (so the UI can note it).
pub fn filter<'a>(
    entries: &'a [LocationEntry],
    query: &str,
    limit: usize,
) -> (Vec<&'a LocationEntry>, bool) {
    let mut out: Vec<&LocationEntry> = Vec::new();
    let mut truncated = false;
    for e in entries.iter().filter(|e| matches(e, query)) {
        if out.len() == limit {
            truncated = true;
            break;
        }
        out.push(e);
    }
    (out, truncated)
}

/// Folds a name to an upper-cased, ASCII-only key for loose matching: Turkish
/// and other Latin diacritics are mapped to their base letter so ASCII typed on
/// the on-screen keyboard matches the localized names, and case is ignored.
/// Non-letter characters are kept as-is.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.trim().chars() {
        let folded = fold_char(ch);
        for u in folded.to_uppercase() {
            out.push(u);
        }
    }
    out
}

/// Maps a single character to an ASCII base letter where there's an obvious
/// fold (covers the Turkish set and the common European diacritics that appear
/// in the API's country/city names); otherwise returns it unchanged.
fn fold_char(ch: char) -> char {
    match ch {
        'ı' | 'İ' | 'î' | 'Î' | 'ï' | 'Ï' => 'i',
        'ş' | 'Ş' => 's',
        'ğ' | 'Ğ' => 'g',
        'ç' | 'Ç' => 'c',
        'ö' | 'Ö' | 'ô' | 'Ô' | 'ó' | 'Ó' | 'ò' | 'Ò' | 'õ' | 'Õ' => 'o',
        'ü' | 'Ü' | 'û' | 'Û' | 'ú' | 'Ú' | 'ù' | 'Ù' => 'u',
        'â' | 'Â' | 'á' | 'Á' | 'à' | 'À' | 'ä' | 'Ä' | 'ã' | 'Ã' | 'å' | 'Å' => 'a',
        'é' | 'É' | 'è' | 'È' | 'ê' | 'Ê' | 'ë' | 'Ë' => 'e',
        'ñ' | 'Ñ' => 'n',
        _ => ch,
    }
}

/// A fully-resolved location the user confirmed, persisted to NVS (as a
/// `serde_json` blob, like the WiFi credentials) so it survives reboots. The
/// `district_id` is the canonical `/vakitler/{id}` key used for every prayer
/// fetch; the names are kept for on-screen display without a re-fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedLocation {
    pub country_id: String,
    pub country_name: String,
    pub city_id: String,
    pub city_name: String,
    pub district_id: String,
    pub district_name: String,
}

impl SelectedLocation {
    /// The Haarlem, Netherlands selection the firmware historically hard-coded
    /// (`ILCE_ID = 13877`). Used as the default when nothing is persisted yet so
    /// a fresh device behaves exactly as before.
    pub fn default_haarlem() -> Self {
        Self {
            country_id: "4".to_string(),
            country_name: "HOLLANDA".to_string(),
            city_id: "721".to_string(),
            city_name: "HOLLANDA".to_string(),
            district_id: "13877".to_string(),
            district_name: "HAARLEM".to_string(),
        }
    }

    /// The label shown in the dashboard header — the most specific (district)
    /// name, upper-cased to match the header's existing mono styling.
    pub fn header_label(&self) -> String {
        self.district_name.trim().to_uppercase()
    }

    /// A selection is only usable if it carries a non-empty district id (the
    /// `/vakitler` key). A blank id from a corrupt/stale blob is treated as
    /// absent so the caller falls back to the default.
    pub fn is_valid(&self) -> bool {
        !self.district_id.trim().is_empty()
    }
}

impl Default for SelectedLocation {
    fn default() -> Self {
        Self::default_haarlem()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_country_rows() {
        let json = br#"[{"UlkeAdi":"HOLLANDA","UlkeAdiEn":"NETHERLANDS","UlkeID":"4"}]"#;
        let out = parse_entries(json).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "HOLLANDA");
        assert_eq!(out[0].name_en, "NETHERLANDS");
        assert_eq!(out[0].id, "4");
    }

    #[test]
    fn parses_city_and_district_rows_with_same_type() {
        let city = br#"[{"SehirAdi":"ADANA","SehirAdiEn":"ADANA","SehirID":"500"}]"#;
        let district = br#"[{"IlceAdi":"HAARLEM","IlceAdiEn":"HAARLEM","IlceID":"13877"}]"#;
        assert_eq!(parse_entries(city).unwrap()[0].id, "500");
        assert_eq!(parse_entries(district).unwrap()[0].id, "13877");
    }

    #[test]
    fn empty_array_parses_to_empty_vec() {
        assert!(parse_entries(b"[]").unwrap().is_empty());
    }

    #[test]
    fn malformed_body_is_an_error() {
        assert!(parse_entries(b"not json").is_err());
    }

    #[test]
    fn missing_english_name_defaults_to_empty() {
        // Some rows in the wild omit the English name; parsing must not fail.
        let json = r#"[{"IlceAdi":"KÖY","IlceID":"9"}]"#;
        let out = parse_entries(json.as_bytes()).unwrap();
        assert_eq!(out[0].name_en, "");
        assert_eq!(out[0].name, "KÖY");
    }

    fn entry(name: &str, name_en: &str, id: &str) -> LocationEntry {
        LocationEntry {
            name: name.to_string(),
            name_en: name_en.to_string(),
            id: id.to_string(),
        }
    }

    #[test]
    fn ascii_query_matches_diacritic_localized_name() {
        let agri = entry("AĞRI", "AGRI", "503");
        assert!(matches(&agri, "agri"));
        assert!(matches(&agri, "AGRI"));
        // Also matches the localized name folded to ASCII.
        assert!(matches(&agri, "AGR"));
    }

    #[test]
    fn match_is_a_case_insensitive_substring() {
        let ist = entry("İSTANBUL", "ISTANBUL", "539");
        assert!(matches(&ist, "ist"));
        assert!(matches(&ist, "STAN"));
        assert!(!matches(&ist, "xyz"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches(&entry("ANY", "ANY", "1"), ""));
        assert!(matches(&entry("ANY", "ANY", "1"), "   "));
    }

    #[test]
    fn filter_preserves_order_and_caps_at_limit() {
        let entries = vec![
            entry("ALKMAAR", "ALKMAAR", "1"),
            entry("ALMELO", "ALMELO", "2"),
            entry("ALMERE", "ALMERE", "3"),
            entry("HAARLEM", "HAARLEM", "4"),
        ];
        let (out, truncated) = filter(&entries, "AL", 2);
        assert_eq!(out.len(), 2);
        assert!(truncated);
        assert_eq!(out[0].id, "1");
        assert_eq!(out[1].id, "2");

        let (all, truncated) = filter(&entries, "AL", 10);
        assert_eq!(all.len(), 3);
        assert!(!truncated);
    }

    #[test]
    fn display_name_trims_trailing_space() {
        assert_eq!(
            entry("ALBERGEN ", "ALBERGEN ", "1").display_name(),
            "ALBERGEN"
        );
    }

    #[test]
    fn selected_location_round_trips_through_serde_json() {
        let sel = SelectedLocation::default_haarlem();
        let json = serde_json::to_string(&sel).unwrap();
        let back: SelectedLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sel);
        assert_eq!(back.district_id, "13877");
    }

    #[test]
    fn default_matches_the_legacy_hardcoded_haarlem_district() {
        let sel = SelectedLocation::default();
        assert_eq!(sel.district_id, "13877");
        assert_eq!(sel.header_label(), "HAARLEM");
        assert!(sel.is_valid());
    }

    #[test]
    fn blank_district_id_is_invalid() {
        let sel = SelectedLocation {
            district_id: "  ".to_string(),
            ..SelectedLocation::default()
        };
        assert!(!sel.is_valid());
    }
}
