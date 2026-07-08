//! Pure, host-testable WiFi credential model and validation.
//!
//! Kept hardware-free (like the rest of this crate) so the SSID/PSK validation
//! rules can be unit tested on a plain host toolchain, mirroring
//! `touch_calibration.rs`. The firmware side — the NVS blob persistence and the
//! on-screen setup UI — lives in `src/wifi_credentials.rs` and
//! `src/wifi_setup.rs` and re-exports [`WifiCredentials`] from here.
//!
//! Persisted as a `serde_json` blob (see `src/wifi_credentials.rs`): unlike the
//! float-carrying touch-calibration record, this holds only strings, and
//! `cache.rs` already proves strings round-trip fine through the Xtensa
//! `serde_json` codegen (the codegen bug is float-specific).

use serde::{Deserialize, Serialize};

/// A WiFi station SSID fits in 32 bytes (802.11), matching the
/// `heapless::String<32>` the esp-idf `ClientConfiguration` uses.
pub const SSID_MAX_LEN: usize = 32;

/// WPA2-Personal passphrase bounds. 8–63 printable characters per the
/// WPA-Personal spec; an empty passphrase denotes an open (unsecured) network.
pub const PSK_MIN_LEN: usize = 8;
pub const PSK_MAX_LEN: usize = 63;

/// The credentials for a single WiFi network, persisted to NVS and used to
/// configure the station interface at boot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiCredentials {
    pub ssid: String,
    /// The WPA2-Personal passphrase, or empty for an open network.
    pub psk: String,
}

/// Why a [`WifiCredentials`] failed [`WifiCredentials::validate`]. Kept as a
/// plain enum so the UI layer can map each case to a localized message without
/// this crate depending on the translation tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// The SSID was empty — an SSID is always required.
    SsidEmpty,
    /// The SSID exceeded [`SSID_MAX_LEN`] bytes.
    SsidTooLong,
    /// A non-empty passphrase was shorter than [`PSK_MIN_LEN`] characters.
    PskTooShort,
    /// The passphrase exceeded [`PSK_MAX_LEN`] characters.
    PskTooLong,
}

impl WifiCredentials {
    /// Builds credentials from any string-like SSID/PSK. Does not validate —
    /// call [`Self::validate`] before persisting or connecting.
    pub fn new(ssid: impl Into<String>, psk: impl Into<String>) -> Self {
        Self {
            ssid: ssid.into(),
            psk: psk.into(),
        }
    }

    /// `true` for an open (passphrase-less) network.
    pub fn is_open(&self) -> bool {
        self.psk.is_empty()
    }

    /// Checks the SSID/PSK against the 802.11 / WPA2-Personal length limits.
    ///
    /// SSID length is counted in bytes (the on-wire limit); passphrase length
    /// in characters (the WPA-Personal spec is defined over printable
    /// characters). An empty passphrase is accepted and denotes an open
    /// network.
    pub fn validate(&self) -> Result<(), CredentialError> {
        if self.ssid.is_empty() {
            return Err(CredentialError::SsidEmpty);
        }
        if self.ssid.len() > SSID_MAX_LEN {
            return Err(CredentialError::SsidTooLong);
        }
        if !self.psk.is_empty() {
            let psk_chars = self.psk.chars().count();
            if psk_chars < PSK_MIN_LEN {
                return Err(CredentialError::PskTooShort);
            }
            if psk_chars > PSK_MAX_LEN {
                return Err(CredentialError::PskTooLong);
            }
        }
        Ok(())
    }

    /// Convenience predicate over [`Self::validate`].
    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_typical_wpa2_network() {
        let c = WifiCredentials::new("HomeNet", "correcthorse");
        assert_eq!(c.validate(), Ok(()));
        assert!(c.is_valid());
        assert!(!c.is_open());
    }

    #[test]
    fn accepts_open_network_with_empty_psk() {
        let c = WifiCredentials::new("CafeGuest", "");
        assert_eq!(c.validate(), Ok(()));
        assert!(c.is_open());
    }

    #[test]
    fn rejects_empty_ssid() {
        assert_eq!(
            WifiCredentials::new("", "password1").validate(),
            Err(CredentialError::SsidEmpty)
        );
    }

    #[test]
    fn rejects_over_long_ssid() {
        let ssid = "x".repeat(SSID_MAX_LEN + 1);
        assert_eq!(
            WifiCredentials::new(ssid, "password1").validate(),
            Err(CredentialError::SsidTooLong)
        );
        // Exactly at the limit is fine.
        let ssid_max = "x".repeat(SSID_MAX_LEN);
        assert!(WifiCredentials::new(ssid_max, "password1").is_valid());
    }

    #[test]
    fn enforces_wpa2_passphrase_bounds() {
        // One char below the minimum.
        let short = "x".repeat(PSK_MIN_LEN - 1);
        assert_eq!(
            WifiCredentials::new("Net", short).validate(),
            Err(CredentialError::PskTooShort)
        );
        // At the minimum and maximum are both fine.
        assert!(WifiCredentials::new("Net", "x".repeat(PSK_MIN_LEN)).is_valid());
        assert!(WifiCredentials::new("Net", "x".repeat(PSK_MAX_LEN)).is_valid());
        // One char above the maximum.
        let long = "x".repeat(PSK_MAX_LEN + 1);
        assert_eq!(
            WifiCredentials::new("Net", long).validate(),
            Err(CredentialError::PskTooLong)
        );
    }

    #[test]
    fn round_trips_through_serde_json() {
        // The firmware persists this as a serde_json blob; confirm the shape is
        // stable so a stored credential reloads across firmware builds.
        let c = WifiCredentials::new("HomeNet", "correcthorse");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"ssid":"HomeNet","psk":"correcthorse"}"#);
        let back: WifiCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }
}
