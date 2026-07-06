//! Pure `DayTimes` model and parsing/formatting helpers for prayer times, as
//! returned by the ezanvakti.emushaf.net API. Kept free of any HTTP/ESP-IDF
//! dependency so it can be unit tested on a plain host toolchain; the actual
//! HTTPS fetch lives in the firmware crate's `src/prayer.rs`, which
//! re-exports [`DayTimes`] from here.

use serde::{Deserialize, Serialize};

use crate::time_utils::parse_hhmm_to_seconds;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DayTimes {
        DayTimes {
            date: "06.07.2024".to_string(),
            hijri_date: "30.12.1445".to_string(),
            imsak: "03:15".to_string(),
            ogle: "13:25".to_string(),
            ikindi: "17:10".to_string(),
            aksam: "21:45".to_string(),
            yatsi: "23:30".to_string(),
        }
    }

    #[test]
    fn prayers_are_in_order_and_exclude_sunrise() {
        let names: Vec<&str> = sample().prayers().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["İMSAK", "ÖĞLE", "İKİNDİ", "AKŞAM", "YATSI"]);
    }

    #[test]
    fn prayer_seconds_parses_each_hhmm() {
        let seconds = sample().prayer_seconds();
        assert_eq!(seconds[0], ("İMSAK", 3 * 3600 + 15 * 60));
        assert_eq!(seconds[1], ("ÖĞLE", 13 * 3600 + 25 * 60));
        assert_eq!(seconds[4], ("YATSI", 23 * 3600 + 30 * 60));
    }

    #[test]
    fn prayer_seconds_defaults_to_zero_on_bad_input() {
        let mut day = sample();
        day.imsak = "not-a-time".to_string();
        assert_eq!(day.prayer_seconds()[0], ("İMSAK", 0));
    }

    #[test]
    fn day_times_deserializes_from_api_shaped_json() {
        let json = r#"{
            "MiladiTarihKisa": "06.07.2024",
            "HicriTarihKisa": "30.12.1445",
            "Imsak": "03:15",
            "Gunes": "05:20",
            "Ogle": "13:25",
            "Ikindi": "17:10",
            "Aksam": "21:45",
            "Yatsi": "23:30"
        }"#;
        let day: DayTimes = serde_json::from_str(json).expect("valid DayTimes JSON");
        assert_eq!(day.date, "06.07.2024");
        assert_eq!(day.imsak, "03:15");
        assert_eq!(day.yatsi, "23:30");
    }

    /// The firmware's `fetch_month()` (in `src/prayer.rs`) deserializes the
    /// `GET /vakitler/{ilceId}` response body straight into `Vec<DayTimes>` —
    /// i.e. the real API contract is a top-level JSON *array* of day objects
    /// (each with a few extra fields we don't model, like `"Gunes"`/sunrise).
    /// The actual live endpoint can't be hit from a unit test (that's
    /// ESP-IDF-only code, and unit tests shouldn't depend on an external
    /// service being reachable) — but this fixture, shaped exactly like the
    /// real endpoint's payload, pins down the parsing contract our code
    /// relies on.
    #[test]
    fn day_times_vec_deserializes_from_api_shaped_array() {
        let json = r#"[
            {
                "MiladiTarihKisa": "06.07.2024",
                "HicriTarihKisa": "30.12.1445",
                "Imsak": "03:15",
                "Gunes": "05:20",
                "Ogle": "13:25",
                "Ikindi": "17:10",
                "Aksam": "21:45",
                "Yatsi": "23:30"
            },
            {
                "MiladiTarihKisa": "07.07.2024",
                "HicriTarihKisa": "01.01.1446",
                "Imsak": "03:16",
                "Gunes": "05:21",
                "Ogle": "13:25",
                "Ikindi": "17:09",
                "Aksam": "21:44",
                "Yatsi": "23:28"
            }
        ]"#;
        let days: Vec<DayTimes> = serde_json::from_str(json).expect("valid DayTimes[] JSON");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].date, "06.07.2024");
        assert_eq!(days[1].date, "07.07.2024");
        assert_eq!(days[1].hijri_date, "01.01.1446");
    }
}
