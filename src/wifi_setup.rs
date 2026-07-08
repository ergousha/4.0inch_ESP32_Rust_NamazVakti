//! On-device WiFi provisioning UI: a scan-driven network picker and an
//! on-screen keyboard for the passphrase (and for manually typing a hidden
//! SSID).
//!
//! This reuses the settings-screen interaction pattern directly (issue #11):
//! geometry constants are shared between the drawing and the hit-testing
//! helpers so a tap always maps back to exactly what was drawn, and each screen
//! is a "draw once, then poll `sample_position` in a debounced loop" flow — the
//! same idiom as [`crate::run_settings_screen`] and the calibration wizard.
//!
//! Everything here is hardware-facing (it scans with `EspWifi` and drives the
//! panel); the credential model and its validation are pure, host-testable code
//! in `namaz-vakti-logic` (`wifi_credentials`). The caller ([`crate::main`]) is
//! responsible for actually connecting with, and persisting, the returned
//! credentials.

use std::thread::sleep;
use std::time::Duration;

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
};
use embedded_hal::spi::SpiDevice;
use embedded_svc::wifi::AuthMethod;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};

use namaz_vakti_logic::language::{self, Language, Msg};
use namaz_vakti_logic::wifi_credentials::{PSK_MAX_LEN, SSID_MAX_LEN};

use crate::text::{self, HAlign};
use crate::touch::Xpt2046;
use crate::touch_calibration::Calibration;
use crate::wifi_credentials::WifiCredentials;
use crate::{settings_screen, Rgb565};

/// Max scan results shown in the picker (strongest first). Anything beyond this
/// is dropped rather than scrolled — a home/office rarely needs more, and
/// paginating a resistive-touch list adds complexity out of proportion to the
/// benefit. Truncation is logged so it isn't silent.
const MAX_NETWORKS: usize = 6;

// --- Network picker geometry (shared by draw + hit-test) ---
const ROW_X: i32 = 8;
const ROW_W: u32 = 464;
const ROW_H: u32 = 30;
const ROW_GAP: i32 = 3;
const LIST_TOP: i32 = 42;

fn row_y(index: usize) -> i32 {
    LIST_TOP + index as i32 * (ROW_H as i32 + ROW_GAP)
}

/// A deduplicated, display-ready scan result.
struct ScannedAp {
    ssid: String,
    open: bool,
    signal: i8,
}

/// What the user tapped on the network picker.
enum PickerChoice {
    /// The Nth network in the displayed list.
    Network(usize),
    /// "Enter manually" — type a (possibly hidden) SSID by hand.
    Manual,
    /// "Rescan" — run the scan again.
    Rescan,
    /// The back arrow — abandon setup (keep any existing credentials).
    Cancel,
}

/// Runs the full provisioning flow to completion: scan → pick (or enter
/// manually) → passphrase, looping on invalid input or a rescan. Returns the
/// chosen (validated) credentials, or `None` if the user backed out.
///
/// The returned credentials are validated but *not* connected or persisted —
/// that is the caller's job, so a failed connection can drop straight back into
/// this flow with the same panel/touch handles.
pub fn run_setup<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    lang: Language,
) -> anyhow::Result<Option<WifiCredentials>>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    // Scanning requires the station interface to be started.
    if !wifi.is_started().unwrap_or(false) {
        if let Err(e) = wifi.start() {
            log::warn!("Failed to start WiFi before scan: {e:?}");
        }
    }

    loop {
        crate::draw_status(display, &[language::text(lang, Msg::WifiScanning)], lang)?;
        let aps = scan_networks(wifi);

        match run_picker(display, touch, calibration, lang, &aps)? {
            PickerChoice::Cancel => return Ok(None),
            PickerChoice::Rescan => continue,
            PickerChoice::Network(i) => {
                let ap = &aps[i];
                let ssid = ap.ssid.clone();
                if ap.open {
                    // Open network — no passphrase needed.
                    let creds = WifiCredentials::new(ssid, "");
                    if creds.is_valid() {
                        return Ok(Some(creds));
                    }
                    continue;
                }
                match run_text_entry(
                    display,
                    touch,
                    calibration,
                    lang,
                    Msg::WifiEnterPassword,
                    Some(&ssid),
                    PSK_MAX_LEN,
                )? {
                    None => continue, // back arrow → return to the picker
                    Some(psk) => {
                        if let Some(creds) = finish(display, lang, ssid, psk) {
                            return Ok(Some(creds));
                        }
                    }
                }
            }
            PickerChoice::Manual => {
                let ssid = match run_text_entry(
                    display,
                    touch,
                    calibration,
                    lang,
                    Msg::WifiEnterSsid,
                    None,
                    SSID_MAX_LEN,
                )? {
                    None => continue,
                    Some(s) if s.is_empty() => continue,
                    Some(s) => s,
                };
                let psk = match run_text_entry(
                    display,
                    touch,
                    calibration,
                    lang,
                    Msg::WifiEnterPassword,
                    Some(&ssid),
                    PSK_MAX_LEN,
                )? {
                    None => continue,
                    Some(p) => p,
                };
                if let Some(creds) = finish(display, lang, ssid, psk) {
                    return Ok(Some(creds));
                }
            }
        }
    }
}

/// Validates the assembled credentials; on success returns them, on failure
/// flashes the reason and returns `None` so the caller re-runs the flow.
fn finish<D>(display: &mut D, lang: Language, ssid: String, psk: String) -> Option<WifiCredentials>
where
    D: DrawTarget<Color = Rgb565>,
{
    let creds = WifiCredentials::new(ssid, psk);
    match creds.validate() {
        Ok(()) => Some(creds),
        Err(_) => {
            // The only reachable failure from the keyboard flow is a too-short
            // passphrase (the SSID is length-capped as it's typed, and empty
            // SSIDs are filtered before this point).
            let _ = crate::draw_status(
                display,
                &[language::text(lang, Msg::WifiPasswordTooShort)],
                lang,
            );
            sleep(Duration::from_millis(1600));
            None
        }
    }
}

/// Scans for access points and returns a deduplicated, display-ready list:
/// hidden (empty-SSID) networks dropped, duplicates collapsed to their
/// strongest sighting, sorted by signal strength, capped at [`MAX_NETWORKS`].
fn scan_networks(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Vec<ScannedAp> {
    let raw = match wifi.scan() {
        Ok(list) => list,
        Err(e) => {
            log::warn!("WiFi scan failed: {e:?}");
            return Vec::new();
        }
    };

    let mut out: Vec<ScannedAp> = Vec::new();
    for ap in raw {
        let ssid = ap.ssid.as_str().to_string();
        if ssid.is_empty() {
            continue; // hidden SSID — reachable via "Enter manually"
        }
        // An access point is "open" when it advertises no auth or explicitly
        // AuthMethod::None.
        let open = matches!(ap.auth_method, None | Some(AuthMethod::None));
        match out.iter_mut().find(|e| e.ssid == ssid) {
            Some(existing) => {
                if ap.signal_strength > existing.signal {
                    existing.signal = ap.signal_strength;
                    existing.open = open;
                }
            }
            None => out.push(ScannedAp {
                ssid,
                open,
                signal: ap.signal_strength,
            }),
        }
    }

    // Strongest first.
    out.sort_by_key(|a| std::cmp::Reverse(a.signal));
    if out.len() > MAX_NETWORKS {
        log::info!(
            "Scan found {} networks; showing the {MAX_NETWORKS} strongest",
            out.len()
        );
        out.truncate(MAX_NETWORKS);
    }
    out
}

// --- Network picker ---

fn run_picker<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    lang: Language,
    aps: &[ScannedAp],
) -> anyhow::Result<PickerChoice>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    draw_picker(display, lang, aps)?;
    wait_for_release(touch);

    let mut press_handled = false;
    loop {
        match touch.sample_position() {
            Ok(Some((xr, yr))) => {
                if !press_handled {
                    press_handled = true;
                    let (x, y) = calibration.to_screen(xr, yr);
                    if settings_screen::point_in_icon(x, y) {
                        return Ok(PickerChoice::Cancel);
                    }
                    if let Some(choice) = picker_hit(x, y, aps.len()) {
                        return Ok(choice);
                    }
                }
            }
            Ok(None) => press_handled = false,
            Err(e) => log::warn!("Touch read failed in WiFi picker: {e:?}"),
        }
        sleep(Duration::from_millis(40));
    }
}

/// Maps a calibrated tap to a picker row. Row layout: `aps.len()` network rows,
/// then "Enter manually", then "Rescan".
fn picker_hit(x: i32, y: i32, n_aps: usize) -> Option<PickerChoice> {
    if x < ROW_X || x >= ROW_X + ROW_W as i32 {
        return None;
    }
    for i in 0..(n_aps + 2) {
        let ry = row_y(i);
        if y >= ry && y < ry + ROW_H as i32 {
            return Some(if i < n_aps {
                PickerChoice::Network(i)
            } else if i == n_aps {
                PickerChoice::Manual
            } else {
                PickerChoice::Rescan
            });
        }
    }
    None
}

fn draw_picker<D>(display: &mut D, lang: Language, aps: &[ScannedAp]) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(crate::col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    settings_screen::draw_back_icon(display)?;

    text::draw_line(
        display,
        language::text(lang, Msg::WifiSelectNetwork),
        Point::new(240, 24),
        HAlign::Center,
        crate::col_text(),
        lang,
        text::Size::Medium,
    )?;

    if aps.is_empty() {
        // No visible networks — still offer manual entry + rescan below the note.
        text::draw_line(
            display,
            language::text(lang, Msg::WifiNoNetworks),
            Point::new(240, row_y(0) + 20),
            HAlign::Center,
            crate::col_dim(),
            lang,
            text::Size::Small,
        )?;
    }

    for (i, ap) in aps.iter().enumerate() {
        draw_row(display, i, &ap.ssid, Language::default(), false)?;
        if !ap.open {
            draw_lock(
                display,
                ROW_X + ROW_W as i32 - 18,
                row_y(i) + ROW_H as i32 / 2,
            )?;
        }
    }
    // Action rows. Their labels are localized, so they render in the active
    // script (the SSID rows above stay Latin — SSIDs aren't translatable).
    draw_row(
        display,
        aps.len(),
        language::text(lang, Msg::WifiEnterManually),
        lang,
        true,
    )?;
    draw_row(
        display,
        aps.len() + 1,
        language::text(lang, Msg::WifiRescan),
        lang,
        true,
    )?;
    Ok(())
}

/// Draws one full-width picker row. `accent` tints the action rows so they read
/// as controls rather than networks.
fn draw_row<D>(
    display: &mut D,
    index: usize,
    label: &str,
    label_lang: Language,
    accent: bool,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let y = row_y(index);
    let border = PrimitiveStyleBuilder::new()
        .stroke_color(if accent {
            crate::col_accent()
        } else {
            crate::col_dim()
        })
        .stroke_width(1)
        .fill_color(crate::col_card_bg())
        .build();
    Rectangle::new(Point::new(ROW_X, y), Size::new(ROW_W, ROW_H))
        .into_styled(border)
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    let (tx, align) = if label_lang.is_rtl() {
        (ROW_X + ROW_W as i32 - 12, HAlign::Right)
    } else {
        (ROW_X + 12, HAlign::Left)
    };
    text::draw_line(
        display,
        label,
        Point::new(tx, y + 20),
        align,
        if accent {
            crate::col_accent()
        } else {
            crate::col_text()
        },
        label_lang,
        text::Size::Small,
    )?;
    Ok(())
}

/// Small padlock glyph marking a secured network, drawn from primitives (the
/// mono font has no lock glyph). `cx`/`cy` is the lock's centre.
fn draw_lock<D>(display: &mut D, cx: i32, cy: i32) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = crate::col_dim();
    // Body.
    Rectangle::new(Point::new(cx - 5, cy - 1), Size::new(10, 8))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    // Shackle (outline arch above the body).
    Rectangle::new(Point::new(cx - 3, cy - 6), Size::new(6, 6))
        .into_styled(PrimitiveStyle::with_stroke(color, 1))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    Ok(())
}

// --- On-screen keyboard ---

#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Lower,
    Upper,
    Symbols,
}

#[derive(Clone, Copy)]
enum KeyAction {
    Char(char),
    Shift,
    ToggleSymbols,
    Backspace,
    Space,
    Done,
}

struct PlacedKey {
    action: KeyAction,
    label: String,
    /// Script the label is shaped in — Latin (English) for the character keys
    /// so ASCII always renders, the active language for the localized controls.
    label_lang: Language,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

const KB_TOP: i32 = 76;
const KEY_W: u32 = 44;
const KEY_H: u32 = 44;
const KEY_HGAP: i32 = 3;
const KEY_VGAP: i32 = 4;
const KB_MARGIN: i32 = 6;

fn key_pitch() -> i32 {
    KEY_H as i32 + KEY_VGAP
}

/// Builds the full laid-out key list for a layer, control row included. This is
/// the single source of truth shared by drawing and hit-testing.
fn layout_keys(layer: Layer, lang: Language) -> Vec<PlacedKey> {
    let rows: [&str; 4] = match layer {
        Layer::Lower => ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"],
        Layer::Upper => ["1234567890", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"],
        Layer::Symbols => ["1234567890", "!@#$%^&*()", "-_=+[]{}\\|", ";:'\",.<>/?"],
    };

    let mut keys = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let n = row.chars().count() as i32;
        let row_w = n * KEY_W as i32 + (n - 1) * KEY_HGAP;
        let start_x = (480 - row_w) / 2;
        let y = KB_TOP + r as i32 * key_pitch();
        for (c, ch) in row.chars().enumerate() {
            keys.push(PlacedKey {
                action: KeyAction::Char(ch),
                label: ch.to_string(),
                label_lang: Language::English,
                x: start_x + c as i32 * (KEY_W as i32 + KEY_HGAP),
                y,
                w: KEY_W,
                h: KEY_H,
            });
        }
    }

    // Control row. A width of 0 marks the flex key (space), which absorbs the
    // leftover width so the row always spans edge-to-edge.
    let done = language::text(lang, Msg::KeyDone).to_string();
    let space = language::text(lang, Msg::KeySpace).to_string();
    let controls: Vec<(KeyAction, String, Language, u32)> = match layer {
        Layer::Symbols => vec![
            (
                KeyAction::ToggleSymbols,
                "ABC".into(),
                Language::English,
                58,
            ),
            (KeyAction::Space, space, lang, 0),
            (KeyAction::Backspace, "del".into(), Language::English, 58),
            (KeyAction::Done, done, lang, 64),
        ],
        _ => vec![
            (
                KeyAction::ToggleSymbols,
                "123".into(),
                Language::English,
                58,
            ),
            (KeyAction::Shift, "Aa".into(), Language::English, 58),
            (KeyAction::Space, space, lang, 0),
            (KeyAction::Backspace, "del".into(), Language::English, 58),
            (KeyAction::Done, done, lang, 64),
        ],
    };

    let avail = 480 - 2 * KB_MARGIN;
    let n = controls.len() as i32;
    let fixed: i32 = controls.iter().map(|(_, _, _, w)| *w as i32).sum();
    let flex = (avail - fixed - (n - 1) * KEY_HGAP).max(KEY_W as i32);
    let y = KB_TOP + 4 * key_pitch();
    let mut x = KB_MARGIN;
    for (action, label, label_lang, w) in controls {
        let w = if w == 0 { flex as u32 } else { w };
        keys.push(PlacedKey {
            action,
            label,
            label_lang,
            x,
            y,
            w,
            h: KEY_H,
        });
        x += w as i32 + KEY_HGAP;
    }
    keys
}

/// Runs a single text-entry screen (SSID or passphrase). Returns the typed
/// string on "done", or `None` if the user tapped the back arrow.
fn run_text_entry<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &Calibration,
    lang: Language,
    title: Msg,
    context_ssid: Option<&str>,
    max_len: usize,
) -> anyhow::Result<Option<String>>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: SpiDevice,
{
    let mut buf = String::new();
    let mut layer = Layer::Lower;

    draw_keyboard(display, lang, title, context_ssid, &buf, layer)?;
    wait_for_release(touch);

    let mut press_handled = false;
    loop {
        match touch.sample_position() {
            Ok(Some((xr, yr))) => {
                if !press_handled {
                    press_handled = true;
                    let (x, y) = calibration.to_screen(xr, yr);
                    if settings_screen::point_in_icon(x, y) {
                        return Ok(None);
                    }
                    if let Some(action) = hit_key(x, y, layer, lang) {
                        match action {
                            KeyAction::Char(c) => {
                                if buf.chars().count() < max_len {
                                    buf.push(c);
                                    draw_text_field(display, lang, title, context_ssid, &buf)?;
                                }
                            }
                            KeyAction::Space => {
                                if buf.chars().count() < max_len {
                                    buf.push(' ');
                                    draw_text_field(display, lang, title, context_ssid, &buf)?;
                                }
                            }
                            KeyAction::Backspace => {
                                buf.pop();
                                draw_text_field(display, lang, title, context_ssid, &buf)?;
                            }
                            KeyAction::Shift => {
                                layer = match layer {
                                    Layer::Lower => Layer::Upper,
                                    Layer::Upper => Layer::Lower,
                                    Layer::Symbols => Layer::Lower,
                                };
                                draw_keyboard(display, lang, title, context_ssid, &buf, layer)?;
                            }
                            KeyAction::ToggleSymbols => {
                                layer = if layer == Layer::Symbols {
                                    Layer::Lower
                                } else {
                                    Layer::Symbols
                                };
                                draw_keyboard(display, lang, title, context_ssid, &buf, layer)?;
                            }
                            KeyAction::Done => return Ok(Some(buf)),
                        }
                    }
                }
            }
            Ok(None) => press_handled = false,
            Err(e) => log::warn!("Touch read failed in keyboard: {e:?}"),
        }
        sleep(Duration::from_millis(40));
    }
}

/// Maps a calibrated tap to the key action under it, if any.
fn hit_key(x: i32, y: i32, layer: Layer, lang: Language) -> Option<KeyAction> {
    layout_keys(layer, lang)
        .into_iter()
        .find(|k| x >= k.x && x < k.x + k.w as i32 && y >= k.y && y < k.y + k.h as i32)
        .map(|k| k.action)
}

/// Full keyboard repaint: text field + every key for `layer`.
fn draw_keyboard<D>(
    display: &mut D,
    lang: Language,
    title: Msg,
    context_ssid: Option<&str>,
    buf: &str,
    layer: Layer,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(crate::col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    settings_screen::draw_back_icon(display)?;
    draw_field_contents(display, lang, title, context_ssid, buf)?;

    for key in layout_keys(layer, lang) {
        draw_key(display, &key)?;
    }
    Ok(())
}

/// Repaints just the title/field region (used on every keystroke so the keys
/// don't flicker).
fn draw_text_field<D>(
    display: &mut D,
    lang: Language,
    title: Msg,
    context_ssid: Option<&str>,
    buf: &str,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    Rectangle::new(Point::new(0, 0), Size::new(480, (KB_TOP - 2) as u32))
        .into_styled(PrimitiveStyle::with_fill(crate::col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    // The back icon lives in this band, so repaint it after the clear.
    settings_screen::draw_back_icon(display)?;
    draw_field_contents(display, lang, title, context_ssid, buf)
}

const FIELD_X: i32 = 8;
const FIELD_Y: i32 = 34;
const FIELD_W: u32 = 464;
const FIELD_H: u32 = 32;

fn draw_field_contents<D>(
    display: &mut D,
    lang: Language,
    title: Msg,
    context_ssid: Option<&str>,
    buf: &str,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Title (localized). When entering a passphrase, the network name is shown
    // as context to the right of the title in Latin.
    text::draw_line(
        display,
        language::text(lang, title),
        Point::new(FIELD_X, 22),
        HAlign::Left,
        crate::col_text(),
        lang,
        text::Size::Small,
    )?;
    if let Some(ssid) = context_ssid {
        text::draw_line(
            display,
            ssid,
            Point::new(FIELD_X + FIELD_W as i32, 22),
            HAlign::Right,
            crate::col_dim(),
            Language::default(),
            text::Size::Small,
        )?;
    }

    // Field box.
    Rectangle::new(Point::new(FIELD_X, FIELD_Y), Size::new(FIELD_W, FIELD_H))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .stroke_color(crate::col_accent())
                .stroke_width(1)
                .fill_color(crate::col_card_bg())
                .build(),
        )
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    // Entered text (Latin/mono). If it overflows the box, show the tail so the
    // most-recently-typed characters stay visible. A trailing caret marks the
    // cursor. SSIDs/passphrases from the keyboard are ASCII, so byte length ==
    // char count and slicing on a byte boundary is safe.
    let visible = (FIELD_W as usize - 16) / 9;
    let shown: &str = if buf.len() > visible {
        &buf[buf.len() - visible..]
    } else {
        buf
    };
    let with_caret = format!("{shown}_");
    text::draw_line(
        display,
        &with_caret,
        Point::new(FIELD_X + 8, FIELD_Y + 22),
        HAlign::Left,
        crate::col_text(),
        Language::English,
        text::Size::Small,
    )?;
    Ok(())
}

fn draw_key<D>(display: &mut D, key: &PlacedKey) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Control keys (anything that isn't a plain character) get the accent
    // border so they stand apart from the letter grid.
    let is_control = !matches!(key.action, KeyAction::Char(_));
    let border = PrimitiveStyleBuilder::new()
        .stroke_color(if is_control {
            crate::col_accent()
        } else {
            crate::col_dim()
        })
        .stroke_width(1)
        .fill_color(crate::col_card_bg())
        .build();
    Rectangle::new(Point::new(key.x, key.y), Size::new(key.w, key.h))
        .into_styled(border)
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    text::draw_line(
        display,
        &key.label,
        Point::new(key.x + key.w as i32 / 2, key.y + key.h as i32 / 2 + 5),
        HAlign::Center,
        crate::col_text(),
        key.label_lang,
        text::Size::Small,
    )?;
    Ok(())
}

/// Blocks until the panel is released, so the tap that opened a screen can't
/// immediately trigger a control on it (the same guard the settings screen uses).
fn wait_for_release<SPI>(touch: &mut Xpt2046<SPI>)
where
    SPI: SpiDevice,
{
    while matches!(touch.is_touched(), Ok(true)) {
        sleep(Duration::from_millis(20));
    }
}
