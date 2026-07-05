//! Pure calendar/timezone arithmetic — no libc `tzset`/`localtime_r` calls, since
//! it's not certain those are exposed through the ESP-IDF Rust bindings on every
//! version. Only the Europe/Amsterdam offset is needed here, computed straight
//! from the EU DST rule (last Sunday of March/October, 01:00 UTC).

pub const SECS_PER_DAY: i64 = 86_400;

/// Civil calendar date from a day count since 1970-01-01 (Howard Hinnant's
/// `civil_from_days` algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Inverse of [`civil_from_days`].
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// 0 = Sunday, ..., 6 = Saturday. Day 0 (1970-01-01) was a Thursday.
fn weekday_from_days(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

const WEEKDAY_NAMES: [&str; 7] = [
    "PAZAR",
    "PAZARTESİ",
    "SALI",
    "ÇARŞAMBA",
    "PERŞEMBE",
    "CUMA",
    "CUMARTESİ",
];

/// UTC-epoch seconds of the last Sunday of `month` (March or October) at 01:00 UTC.
fn last_sunday_01_utc(year: i64, month: u32) -> i64 {
    let days = days_from_civil(year, month, 31);
    let back = weekday_from_days(days);
    (days - back) * SECS_PER_DAY + 3600
}

/// Europe/Amsterdam UTC offset in seconds (3600 = CET, 7200 = CEST) for a UTC
/// epoch timestamp, following the EU DST rule.
pub fn amsterdam_offset_seconds(epoch: i64) -> i64 {
    let days = epoch.div_euclid(SECS_PER_DAY);
    let (year, _, _) = civil_from_days(days);
    let dst_start = last_sunday_01_utc(year, 3);
    let dst_end = last_sunday_01_utc(year, 10);
    if epoch >= dst_start && epoch < dst_end {
        7200
    } else {
        3600
    }
}

/// Local (Amsterdam) wall-clock broken-down time for a UTC epoch timestamp.
pub struct LocalTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    pub fn from_epoch(epoch: i64) -> Self {
        let offset = amsterdam_offset_seconds(epoch);
        let local_epoch = epoch + offset;
        let days = local_epoch.div_euclid(SECS_PER_DAY);
        let sod = local_epoch.rem_euclid(SECS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (sod / 3600) as u32,
            minute: ((sod / 60) % 60) as u32,
            second: (sod % 60) as u32,
        }
    }

    /// Seconds since local midnight.
    pub fn seconds_of_day(&self) -> u32 {
        self.hour * 3600 + self.minute * 60 + self.second
    }

    /// Matches the ezanvakti API's `MiladiTarihKisa` field format ("DD.MM.YYYY").
    pub fn date_key(&self) -> String {
        format_date_key(self.year, self.month, self.day)
    }

    pub fn weekday_name(&self) -> &'static str {
        let days = days_from_civil(self.year, self.month, self.day);
        WEEKDAY_NAMES[weekday_from_days(days) as usize]
    }
}

/// Matches the ezanvakti API's `MiladiTarihKisa` field format ("DD.MM.YYYY").
pub fn format_date_key(year: i64, month: u32, day: u32) -> String {
    format!("{day:02}.{month:02}.{year}")
}

/// Parses an "HH:MM" string into seconds since midnight.
pub fn parse_hhmm_to_seconds(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    Some(h * 3600 + m * 60)
}
