mod cache;
mod framebuf;
mod prayer;
mod segdisplay;
mod settings;
mod settings_screen;
mod text;
mod time_utils;
mod touch;
mod touch_calibration;
mod wifi_credentials;
mod wifi_setup;

use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use embedded_graphics::{
    mono_font::{
        iso_8859_9::{FONT_9X15, FONT_9X18_BOLD},
        MonoTextStyle,
    },
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::{Alignment, Text},
};
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration as WifiConfiguration};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        gpio::PinDriver,
        ledc::{config::TimerConfig as LedcTimerConfig, LedcDriver, LedcTimerDriver},
        peripherals::Peripherals,
        spi::{config::Config as SpiConfig, SpiDeviceDriver, SpiDriver, SpiDriverConfig},
        units::FromValueType,
    },
    nvs::EspDefaultNvsPartition,
    sntp::{EspSntp, SyncStatus},
    wifi::{BlockingWifi, EspWifi},
};
use mipidsi::{
    interface::SpiInterface,
    models::ST7796,
    options::{ColorOrder, Orientation, Rotation},
    Builder,
};

use namaz_vakti_logic::language::{self, Language, Msg};

use framebuf::FrameBuf;
use prayer::DayTimes;
use time_utils::LocalTime;
use touch::Xpt2046;
use wifi_credentials::WifiCredentials;

/// Default backlight brightness as a percentage of max PWM duty. Lower this
/// if you want a dimmer default; the backlight is on GPIO27 via LEDC PWM.
const BACKLIGHT_DUTY_PERCENT: u32 = 100;

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
}

/// Which calendar the header's date is shown in. Chosen from the settings
/// screen and persisted to NVS; the discriminant doubles as the stored byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DateMode {
    Miladi,
    Hijri,
}

impl DateMode {
    fn to_u8(self) -> u8 {
        match self {
            DateMode::Miladi => 0,
            DateMode::Hijri => 1,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => DateMode::Hijri,
            _ => DateMode::Miladi,
        }
    }
}

type Rgb565 = embedded_graphics::pixelcolor::Rgb565;

fn col_bg() -> Rgb565 {
    Rgb565::new(0, 2, 4)
}
fn col_accent() -> Rgb565 {
    Rgb565::new(31, 42, 0)
}
fn col_accent_dark() -> Rgb565 {
    Rgb565::new(2, 2, 0)
}
fn col_text() -> Rgb565 {
    Rgb565::new(27, 54, 27)
}
fn col_dim() -> Rgb565 {
    Rgb565::new(9, 18, 9)
}
fn col_card_bg() -> Rgb565 {
    Rgb565::new(2, 5, 7)
}

// Big seven-segment countdown geometry ("HH:MM:SS"). Sized so all 8 glyphs fit
// within the 480px panel with margins; a RAM framebuffer of `CD_BOX_W` x
// `CD_DIGIT_H` (see [`framebuf::FrameBuf`]) backs it so the whole box flushes in
// one SPI transfer, cheap enough to repaint every second.
const CD_DIGIT_W: u32 = 42;
const CD_DIGIT_H: u32 = 80;
const CD_THICK: u32 = 10;
const CD_GAP: u32 = 10;
const CD_DIGITS_Y: i32 = 74;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- Persisted UI settings (language + header date mode) ---
    // Loaded before the first splash so every status message renders in the
    // saved language; defaults to Türkçe / Miladi on a fresh device.
    let settings_nvs = settings::open(nvs.clone())?;
    let mut settings = settings::load(&settings_nvs);
    log::info!("Loaded settings: {settings:?}");

    // --- Backlight (PWM via LEDC, GPIO27) ---
    let ledc_timer = LedcTimerDriver::new(
        peripherals.ledc.timer0,
        &LedcTimerConfig::new().frequency(5.kHz().into()),
    )?;
    let mut backlight = LedcDriver::new(
        peripherals.ledc.channel0,
        ledc_timer,
        peripherals.pins.gpio27,
    )?;
    backlight.set_duty(backlight.get_max_duty() * BACKLIGHT_DUTY_PERCENT / 100)?;

    // --- Display + touch (ST7796S + XPT2046, sharing SPI2/HSPI; pin map
    // reverse-engineered from the board's C/ESP-IDF/LVGL project, see
    // README.md) ---
    let dc = PinDriver::output(peripherals.pins.gpio2)?;

    let spi_bus = Rc::new(SpiDriver::new(
        peripherals.spi2,
        peripherals.pins.gpio14,       // SCLK
        peripherals.pins.gpio13,       // MOSI
        Some(peripherals.pins.gpio12), // MISO (needed for touch reads)
        &SpiDriverConfig::new(),
    )?);

    // 80MHz write-only matches the board's own C/LVGL driver config
    // (CONFIG_LV_TFT_SPI_CLK_DIVIDER_1 = undivided 80MHz APB clock).
    let display_spi_config = SpiConfig::new()
        .baudrate(80.MHz().into())
        .data_mode(embedded_hal::spi::MODE_0)
        .write_only(true);
    let spi_device = SpiDeviceDriver::new(
        spi_bus.clone(),
        Some(peripherals.pins.gpio15), // CS
        &display_spi_config,
    )?;

    // XPT2046 is a much slower ADC part than the display controller.
    let touch_spi_config = SpiConfig::new()
        .baudrate(2.MHz().into())
        .data_mode(embedded_hal::spi::MODE_0);
    let touch_spi = SpiDeviceDriver::new(
        spi_bus,
        Some(peripherals.pins.gpio33), // CS
        &touch_spi_config,
    )?;
    let mut touch = Xpt2046::new(touch_spi);

    let mut display_buffer = [0u8; 4096];
    let di = SpiInterface::new(spi_device, dc, &mut display_buffer);

    let mut delay = esp_idf_svc::hal::delay::Ets;
    let mut display = Builder::new(ST7796, di)
        .color_order(ColorOrder::Bgr)
        // This panel's column address order is the mirror of mipidsi's default
        // for any rotation (confirmed against the C driver's MADCTL values:
        // 0x48 portrait / 0x28 landscape both have the same fixed-up parity).
        .orientation(Orientation {
            rotation: Rotation::Deg90,
            mirrored: true,
        })
        .init(&mut delay)
        .map_err(|e| anyhow::anyhow!("display init failed: {e:?}"))?;

    display
        .clear(col_bg())
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    draw_status(
        &mut display,
        &[
            language::text(settings.language, Msg::AppTitle),
            language::text(settings.language, Msg::Starting),
        ],
        settings.language,
    )?;

    // --- Touchscreen calibration ---
    // Runs before WiFi so the panel's raw X/Y → screen mapping is ready for any
    // future touch-driven UI. A saved calibration in NVS is reused as-is;
    // holding the screen through this splash for 5s forces a re-calibration
    // (the only trigger for now — no settings-menu entry point yet).
    let touch_cal_nvs = touch_calibration::open(nvs.clone())?;
    let force_recalibrate =
        touch_calibration::recalibration_requested(&mut display, &mut touch, settings.language);
    if force_recalibrate {
        log::info!("Re-calibration gesture detected; clearing saved touch calibration");
        touch_calibration::clear(&touch_cal_nvs);
    }
    let mut calibration = match touch_calibration::load(&touch_cal_nvs) {
        Some(cal) if !force_recalibrate => {
            log::info!("Loaded saved touch calibration: {cal:?}");
            cal
        }
        _ => {
            let outcome = touch_calibration::run_wizard(
                &mut display,
                &mut touch,
                480,
                320,
                settings.language,
            );
            if outcome.should_persist() {
                touch_calibration::save(&touch_cal_nvs, &outcome.calibration());
            }
            outcome.calibration()
        }
    };
    // The dashboard's main loop maps raw touches with
    // `calibration.to_screen(x_raw, y_raw)` to hit-test the header gear icon,
    // which opens the settings screen.
    log::info!("Touch calibration ready: {calibration:?}");
    // The wizard/gesture painted over the splash; restore it before WiFi.
    draw_status(
        &mut display,
        &[
            language::text(settings.language, Msg::AppTitle),
            language::text(settings.language, Msg::Starting),
        ],
        settings.language,
    )?;

    // --- Prayer-time cache: opened before WiFi so a failed reconnect can show
    // a "reconnecting" indicator (there is cached data to fall back on) rather
    // than a first-time "connecting" splash. ---
    let cache_nvs = cache::open(nvs.clone())?;
    let mut days_data = cache::load(&cache_nvs);
    let have_cache = !days_data.is_empty();

    // --- WiFi credentials + connection ---
    // Credentials live in NVS now (set on-device via the setup flow below),
    // replacing the compile-time cfg.toml values. cfg.toml, if present, only
    // *seeds* NVS on first boot, so headless CI/bench builds still connect
    // without a person to tap through setup.
    let wifi_nvs = wifi_credentials::open(nvs.clone())?;
    seed_credentials_from_cfg(&wifi_nvs);
    let creds = wifi_credentials::load(&wifi_nvs);

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))?,
        sys_loop,
    )?;

    // Saved credentials get a bounded retry budget; success continues to
    // NTP/fetch as before. With no saved credentials, or once the budget is
    // exhausted, drop into the blocking on-device setup flow instead of the old
    // reboot-loop-on-failure behavior.
    let mut connected = false;
    if let Some(c) = &creds {
        connected = try_connect(
            &mut display,
            &mut wifi,
            c,
            settings.language,
            WIFI_CONNECT_ATTEMPTS,
            have_cache,
        );
    }
    if !connected {
        provision_and_connect(
            &mut display,
            &mut touch,
            &calibration,
            &mut wifi,
            &wifi_nvs,
            settings.language,
        )?;
    }
    log::info!("WiFi connected");

    // --- Time sync (NTP) ---
    draw_status(
        &mut display,
        &[language::text(settings.language, Msg::TimeSyncing)],
        settings.language,
    )?;
    let sntp = EspSntp::new_default()?;
    let sync_deadline = SystemTime::now() + Duration::from_secs(20);
    while sntp.get_sync_status() != SyncStatus::Completed && SystemTime::now() < sync_deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
    log::info!("SNTP sync status: {:?}", sntp.get_sync_status());

    // --- Prayer time data: the NVS cache (loaded above) lets a reboot show the
    // dashboard immediately instead of blocking on a fresh HTTPS fetch ---
    let mut last_fetch_attempt;
    if days_data.is_empty() {
        draw_status(
            &mut display,
            &[language::text(settings.language, Msg::PrayerDownloading)],
            settings.language,
        )?;
        // A failed initial fetch must not be fatal: with an empty cache there
        // is nothing to show yet, but the device should stay alive, show a
        // status screen, and let the main loop's throttled refresh path keep
        // retrying (DNS/API outages, TLS failures and captive WiFi are all
        // recoverable without a reboot).
        match fetch_with_retry(&mut display, 5, settings.language) {
            Ok(fresh) => {
                cache::save(&cache_nvs, &fresh);
                days_data = fresh;
            }
            Err(e) => {
                log::warn!("Initial prayer-time fetch failed, retrying in background: {e:?}");
                draw_status(
                    &mut display,
                    &[
                        language::text(settings.language, Msg::PrayerFetchFailed),
                        language::text(settings.language, Msg::RetryingInBackground),
                    ],
                    settings.language,
                )?;
            }
        }
        // Record the attempt in both cases so the main loop waits the throttle
        // interval before its next try instead of hammering a failed endpoint.
        last_fetch_attempt = now_epoch();
    } else {
        log::info!(
            "Loaded {} cached prayer-time days from NVS",
            days_data.len()
        );
        last_fetch_attempt = 0;
    }

    // Tracks what's currently on screen so the main loop only repaints the
    // small regions that actually changed instead of the whole panel (a full
    // 480x320 clear+redraw took 100-200ms and was visibly flickering).
    let mut frame_state: Option<FrameState> = None;
    // The header line (wall clock at minute resolution, date, weekday) and the
    // "next vakit" label only change once a minute, so they're gated on the
    // minute to avoid needlessly repainting text every second.
    let mut last_drawn_minute: Option<i64> = None;
    // The big countdown + progress bar tick every second. Rendering goes through
    // `clock_fb` (a RAM framebuffer flushed in one SPI transfer), which is what
    // makes second-resolution updates fast and flicker-free.
    let mut last_drawn_second: Option<i64> = None;
    // Sized to the widest countdown string so the buffer width matches exactly
    // what `draw_big_time` renders (used for centering at flush time).
    let clock_box_w = segdisplay::measure_big_time("00:00:00", CD_DIGIT_W, CD_THICK, CD_GAP);
    let mut clock_fb = FrameBuf::new(clock_box_w, CD_DIGIT_H, col_bg());

    // The header date mode now comes from persisted settings rather than a
    // tap gesture. `press_handled` de-bounces the gear tap so one finger-down
    // opens the settings screen exactly once.
    let mut press_handled = false;

    let mut last_tick = Instant::now() - Duration::from_secs(1); // run the first tick immediately

    // --- Main loop ---
    loop {
        // Touch is polled every iteration (fast) so the gear tap feels
        // responsive; the heavier clock/API-refresh logic below only runs once
        // a second. Touch on the dashboard is used solely to hit-test the gear
        // icon — tapping elsewhere does nothing.
        match touch.sample_position() {
            Ok(Some((x_raw, y_raw))) => {
                if !press_handled {
                    press_handled = true;
                    let (x, y) = calibration.to_screen(x_raw, y_raw);
                    if settings_screen::point_in_icon(x, y) {
                        match run_settings_screen(
                            &mut display,
                            &mut touch,
                            &calibration,
                            &settings_nvs,
                            &mut settings,
                        )? {
                            SettingsExit::Back => {}
                            SettingsExit::Wifi => {
                                // Re-provision on demand: run setup, then connect
                                // + persist on success. A cancel or failed connect
                                // leaves the existing credentials untouched.
                                if let Some(new_creds) = wifi_setup::run_setup(
                                    &mut display,
                                    &mut touch,
                                    &calibration,
                                    &mut wifi,
                                    settings.language,
                                )? {
                                    if try_connect(
                                        &mut display,
                                        &mut wifi,
                                        &new_creds,
                                        settings.language,
                                        WIFI_CONNECT_ATTEMPTS,
                                        false,
                                    ) {
                                        wifi_credentials::save(&wifi_nvs, &new_creds);
                                    } else {
                                        let _ = draw_status(
                                            &mut display,
                                            &[language::text(
                                                settings.language,
                                                Msg::WifiConnectFailed,
                                            )],
                                            settings.language,
                                        );
                                        std::thread::sleep(Duration::from_secs(2));
                                    }
                                }
                            }
                            SettingsExit::Recalibrate => {
                                touch_calibration::clear(&touch_cal_nvs);
                                let outcome = touch_calibration::run_wizard(
                                    &mut display,
                                    &mut touch,
                                    480,
                                    320,
                                    settings.language,
                                );
                                if outcome.should_persist() {
                                    touch_calibration::save(&touch_cal_nvs, &outcome.calibration());
                                }
                                calibration = outcome.calibration();
                            }
                        }
                        // Leaving settings (or a sub-flow) forces a full repaint.
                        frame_state = None;
                        last_drawn_minute = None;
                        last_drawn_second = None;
                    }
                }
            }
            Ok(None) => press_handled = false,
            Err(e) => log::warn!("Touch read failed: {e:?}"),
        }

        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();

            let epoch = now_epoch();
            let local = LocalTime::from_epoch(epoch);
            let today_key = local.date_key();

            let need_refresh = !days_data.iter().any(|d| d.date == today_key);
            if need_refresh && epoch - last_fetch_attempt >= 300 {
                last_fetch_attempt = epoch;
                match prayer::fetch_month() {
                    Ok(fresh) => {
                        log::info!("Prayer data refreshed ({} days)", fresh.len());
                        cache::save(&cache_nvs, &fresh);
                        days_data = fresh;
                    }
                    Err(e) => log::warn!("Prayer data refresh failed: {e:?}"),
                }
            }

            let today_row = days_data.iter().find(|d| d.date == today_key);
            let timeline = build_timeline(&days_data, &local);
            let now_local_secs = day_start_secs(&local) + local.seconds_of_day() as i64;
            let next = timeline.iter().position(|e| e.at > now_local_secs);

            let Some(idx) = next else {
                draw_status(
                    &mut display,
                    &[language::text(settings.language, Msg::PrayerDataMissing)],
                    settings.language,
                )?;
                frame_state = None; // force a full repaint once data is back
                continue;
            };

            let next_entry = &timeline[idx];
            let remaining = next_entry.at - now_local_secs;
            let progress = if idx > 0 {
                let prev_entry = &timeline[idx - 1];
                Some(
                    (now_local_secs - prev_entry.at) as f32
                        / (next_entry.at - prev_entry.at) as f32,
                )
            } else {
                None
            };

            let today_start = day_start_secs(&local);
            let next_is_today = next_entry.at >= today_start
                && next_entry.at < today_start + time_utils::SECS_PER_DAY;
            let next_today_label = if next_is_today {
                Some(next_entry.label)
            } else {
                None
            };

            let day_changed = frame_state
                .as_ref()
                .map(|f| f.today_key != today_key)
                .unwrap_or(true);

            if day_changed {
                draw_static_frame(&mut display, today_row, next_today_label, settings.language)?;
                // Force the header and clock/countdown to repaint too.
                last_drawn_minute = None;
                last_drawn_second = None;
            } else if frame_state.as_ref().unwrap().next_today_label != next_today_label {
                update_card_highlight(
                    &mut display,
                    today_row,
                    frame_state.as_ref().unwrap().next_today_label,
                    next_today_label,
                    settings.language,
                )?;
            }

            // Header + next-vakit label: minute cadence (nothing here changes
            // more often than once a minute).
            let current_minute = now_local_secs.div_euclid(60);
            if last_drawn_minute != Some(current_minute) {
                draw_header(
                    &mut display,
                    &local,
                    today_row,
                    settings.date_mode,
                    next_entry.label,
                    settings.language,
                )?;
                last_drawn_minute = Some(current_minute);
            }

            // Big countdown + progress bar: second cadence via the framebuffer.
            if last_drawn_second != Some(now_local_secs) {
                draw_countdown(&mut display, &mut clock_fb, remaining, progress)?;
                last_drawn_second = Some(now_local_secs);
            }

            frame_state = Some(FrameState {
                today_key,
                next_today_label,
            });
        }

        std::thread::sleep(Duration::from_millis(120));
    }
}

struct FrameState {
    today_key: String,
    next_today_label: Option<&'static str>,
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn day_start_secs(local: &LocalTime) -> i64 {
    time_utils::days_from_civil(local.year, local.month, local.day) * time_utils::SECS_PER_DAY
}

struct TimelineEntry {
    label: &'static str,
    at: i64,
}

/// Builds a chronological (label, absolute-local-seconds) timeline covering
/// yesterday/today/tomorrow, so "next prayer" and "progress" resolve correctly
/// across midnight even right after Yatsi or right before Imsak.
fn build_timeline(days_data: &[DayTimes], local: &LocalTime) -> Vec<TimelineEntry> {
    let mut out = Vec::new();
    let today_days = time_utils::days_from_civil(local.year, local.month, local.day);
    for delta in [-1i64, 0, 1] {
        let (y, m, d) = time_utils::civil_from_days(today_days + delta);
        let key = time_utils::format_date_key(y, m, d);
        if let Some(row) = days_data.iter().find(|r| r.date == key) {
            let day_start = (today_days + delta) * time_utils::SECS_PER_DAY;
            for (label, secs) in row.prayer_seconds() {
                out.push(TimelineEntry {
                    label,
                    at: day_start + secs as i64,
                });
            }
        }
    }
    out
}

fn fetch_with_retry<D>(
    display: &mut D,
    attempts: u32,
    lang: Language,
) -> anyhow::Result<Vec<DayTimes>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut last_err = None;
    for attempt in 1..=attempts {
        match prayer::fetch_month() {
            Ok(days) => return Ok(days),
            Err(e) => {
                log::warn!("Fetch attempt {attempt}/{attempts} failed: {e:?}");
                let _ = draw_status(
                    display,
                    &[
                        language::text(lang, Msg::PrayerDownloadFailed),
                        language::text(lang, Msg::Retrying),
                    ],
                    lang,
                );
                last_err = Some(e);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("unknown fetch error")))
}

/// Attempts per boot/reconnect before falling back to the on-device setup flow.
/// Each attempt is bounded by the driver's own connect/DHCP timeouts, so three
/// attempts is the retry budget referenced in the boot sequence (issue #11) —
/// no more infinite reboot-loop on broken credentials.
const WIFI_CONNECT_ATTEMPTS: u32 = 3;

/// Builds the station configuration for the given credentials. Empty PSK selects
/// an open network (`AuthMethod::None`); anything else is treated as
/// WPA2-Personal (WPA2-Enterprise/EAP is explicitly out of scope).
fn client_config(creds: &WifiCredentials) -> anyhow::Result<WifiConfiguration> {
    let auth_method = if creds.psk.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    Ok(WifiConfiguration::Client(ClientConfiguration {
        ssid: creds
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: creds
            .psk
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method,
        ..Default::default()
    }))
}

/// A single bounded connect attempt: (re)configure, ensure started, associate,
/// wait for an IP. The `BlockingWifi` connect/`wait_netif_up` calls each carry
/// the driver's built-in timeout, so this returns rather than hanging.
fn connect_once(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &WifiCredentials,
) -> anyhow::Result<()> {
    wifi.set_configuration(&client_config(creds)?)?;
    if !wifi.is_started().unwrap_or(false) {
        wifi.start()?;
    }
    wifi.connect()?;
    wifi.wait_netif_up()?;
    Ok(())
}

/// Tries to connect up to `attempts` times, returning `true` on success. Shows a
/// status splash between tries — "reconnecting" when there is cached data to
/// fall back on, otherwise "connecting". The specific disconnect reason isn't
/// surfaced by the blocking API (a wrong password and an unreachable AP both
/// present as a timeout), so a generic failure message is shown.
fn try_connect<D>(
    display: &mut D,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &WifiCredentials,
    lang: Language,
    attempts: u32,
    reconnecting: bool,
) -> bool
where
    D: DrawTarget<Color = Rgb565>,
{
    let heading = if reconnecting {
        Msg::WifiReconnecting
    } else {
        Msg::WifiConnecting
    };
    for attempt in 1..=attempts {
        let _ = draw_status(
            display,
            &[language::text(lang, heading), creds.ssid.as_str()],
            lang,
        );
        match connect_once(wifi, creds) {
            Ok(()) => return true,
            Err(e) => {
                log::warn!("WiFi connect attempt {attempt}/{attempts} failed: {e:?}");
                // Reset the driver state before the next association attempt.
                let _ = wifi.disconnect();
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    let _ = draw_status(
        display,
        &[language::text(lang, Msg::WifiConnectFailed)],
        lang,
    );
    std::thread::sleep(Duration::from_secs(2));
    false
}

/// Blocking on-device provisioning: runs the setup UI, then connects with the
/// entered credentials, persisting them only on a successful connection. Loops
/// on a failed connection (e.g. a mistyped passphrase) so the user gets another
/// try without a reboot. Used at boot when there are no working saved
/// credentials.
fn provision_and_connect<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &touch_calibration::Calibration,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_nvs: &esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>,
    lang: Language,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: embedded_hal::spi::SpiDevice,
{
    loop {
        // At boot there is nothing to fall back on, so a "cancel" (None) just
        // re-opens setup — WiFi is required to make progress.
        let Some(creds) = wifi_setup::run_setup(display, touch, calibration, wifi, lang)? else {
            continue;
        };
        if try_connect(display, wifi, &creds, lang, WIFI_CONNECT_ATTEMPTS, false) {
            wifi_credentials::save(wifi_nvs, &creds);
            return Ok(());
        }
        // Connection failed with fresh credentials — loop back into setup.
    }
}

/// Seeds NVS from the compile-time `cfg.toml` credentials on first boot only, so
/// headless CI/bench builds (with no one to tap through setup) still connect.
/// A no-op once NVS already holds credentials, or when `cfg.toml` is absent
/// (the default empty `CONFIG.wifi_ssid`).
fn seed_credentials_from_cfg(wifi_nvs: &esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>) {
    if CONFIG.wifi_ssid.is_empty() {
        return; // no build-time seed configured
    }
    if wifi_credentials::load(wifi_nvs).is_some() {
        return; // NVS already provisioned — never override the on-device value
    }
    let seed = WifiCredentials::new(CONFIG.wifi_ssid, CONFIG.wifi_psk);
    if seed.is_valid() {
        log::info!("Seeding WiFi credentials from cfg.toml on first boot");
        wifi_credentials::save(wifi_nvs, &seed);
    } else {
        log::warn!("cfg.toml WiFi credentials are invalid; skipping first-boot seed");
    }
}

/// Maps a Turkish prayer label (the stable key used across the timeline) to the
/// active language's prayer name for display.
fn localize_prayer(label: &str, lang: Language) -> &'static str {
    let tr = language::prayer_names(Language::Turkish);
    let idx = tr.iter().position(|n| *n == label).unwrap_or(0);
    language::prayer_names(lang)[idx]
}

/// Maps `LocalTime::weekday_name()` (Turkish) to the active language's weekday.
fn localize_weekday(tr_name: &str, lang: Language) -> &'static str {
    let tr = language::weekday_names(Language::Turkish);
    let idx = tr.iter().position(|n| *n == tr_name).unwrap_or(0);
    language::weekday_names(lang)[idx]
}

/// How the settings screen was left. Language / date-mode toggles are handled
/// in-place; the two system actions (WiFi setup, touch recalibration) need
/// hardware handles the caller owns (the `wifi` driver, the calibration NVS),
/// so they're returned for the main loop to run after the screen closes.
enum SettingsExit {
    Back,
    Wifi,
    Recalibrate,
}

/// Shows the settings screen and processes taps until the user leaves it.
/// Language / date-mode selections are applied immediately, persisted to NVS,
/// and re-rendered so the whole screen reflects the new choice; tapping a
/// system action returns the matching [`SettingsExit`] for the caller to run.
fn run_settings_screen<D, SPI>(
    display: &mut D,
    touch: &mut Xpt2046<SPI>,
    calibration: &touch_calibration::Calibration,
    settings_nvs: &esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>,
    settings: &mut settings::Settings,
) -> anyhow::Result<SettingsExit>
where
    D: DrawTarget<Color = Rgb565>,
    SPI: embedded_hal::spi::SpiDevice,
{
    settings_screen::draw(display, settings)?;
    // Wait for the finger that opened the screen to lift before polling, so the
    // same press can't immediately trigger a control underneath the gear.
    while matches!(touch.is_touched(), Ok(true)) {
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut press_handled = false;
    loop {
        match touch.sample_position() {
            Ok(Some((x_raw, y_raw))) => {
                if !press_handled {
                    press_handled = true;
                    let (x, y) = calibration.to_screen(x_raw, y_raw);
                    match settings_screen::hit_test(x, y) {
                        Some(settings_screen::Hit::Back) => return Ok(SettingsExit::Back),
                        Some(settings_screen::Hit::Wifi) => return Ok(SettingsExit::Wifi),
                        Some(settings_screen::Hit::Recalibrate) => {
                            return Ok(SettingsExit::Recalibrate)
                        }
                        Some(settings_screen::Hit::Language(l)) => {
                            settings.language = l;
                            settings::save_language(settings_nvs, l);
                            settings_screen::draw(display, settings)?;
                        }
                        Some(settings_screen::Hit::DateMode(m)) => {
                            settings.date_mode = m;
                            settings::save_date_mode(settings_nvs, m);
                            settings_screen::draw(display, settings)?;
                        }
                        None => {}
                    }
                }
            }
            Ok(None) => press_handled = false,
            Err(e) => log::warn!("Touch read failed in settings: {e:?}"),
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn draw_status<D>(display: &mut D, lines: &[&str], lang: Language) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    let mut y = 150 - (lines.len() as i32 - 1) * 12;
    for line in lines {
        text::draw_line(
            display,
            line,
            Point::new(240, y),
            text::HAlign::Center,
            col_text(),
            lang,
            text::Size::Medium,
        )?;
        y += 26;
    }
    Ok(())
}

const CARD_MARGIN: i32 = 4;
const CARD_GAP: i32 = 8;
const CARD_W: u32 = 88;
const CARD_H: u32 = 95;
const CARD_Y: i32 = 210;

/// Draws everything that only changes once a day (or on the very first frame):
/// the header separator line and all 5 vakit cards in their correct initial
/// highlight state. This is the only place that clears the *whole* panel.
fn draw_static_frame<D>(
    display: &mut D,
    today: Option<&DayTimes>,
    next_today_label: Option<&str>,
    lang: Language,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    Rectangle::new(Point::new(0, 32), Size::new(480, 1))
        .into_styled(PrimitiveStyle::with_fill(col_dim()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    if let Some(today) = today {
        for i in 0..5 {
            let name = today.prayers()[i].0;
            draw_card(display, today, i, next_today_label == Some(name), lang)?;
        }
    }

    Ok(())
}

/// Redraws only the (at most two) cards whose highlight state changed since
/// the last frame: the previously-highlighted one turns normal, the newly
/// upcoming one turns highlighted.
fn update_card_highlight<D>(
    display: &mut D,
    today: Option<&DayTimes>,
    old_label: Option<&str>,
    new_label: Option<&str>,
    lang: Language,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let Some(today) = today else {
        return Ok(());
    };
    let prayers = today.prayers();
    for (i, (name, _)) in prayers.iter().enumerate() {
        if Some(*name) == old_label {
            draw_card(display, today, i, false, lang)?;
        }
        if Some(*name) == new_label {
            draw_card(display, today, i, true, lang)?;
        }
    }
    Ok(())
}

fn draw_card<D>(
    display: &mut D,
    today: &DayTimes,
    index: usize,
    highlighted: bool,
    lang: Language,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let (_, hhmm) = today.prayers()[index];
    // Prayer names are localized for display; the highlight identity still
    // keys off the stable label from `prayers()`.
    let name = language::prayer_names(lang)[index];
    let x = CARD_MARGIN + index as i32 * (CARD_W as i32 + CARD_GAP);

    let border_style = PrimitiveStyleBuilder::new()
        .stroke_color(if highlighted { col_accent() } else { col_dim() })
        .stroke_width(1)
        .fill_color(if highlighted {
            col_accent()
        } else {
            col_card_bg()
        })
        .build();
    Rectangle::new(Point::new(x, CARD_Y), Size::new(CARD_W, CARD_H))
        .into_styled(border_style)
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    let name_color = if highlighted {
        col_accent_dark()
    } else {
        col_dim()
    };
    let time_style = if highlighted {
        MonoTextStyle::new(&FONT_9X18_BOLD, col_accent_dark())
    } else {
        MonoTextStyle::new(&FONT_9X18_BOLD, col_text())
    };

    text::draw_line(
        display,
        name,
        Point::new(x + CARD_W as i32 / 2, CARD_Y + 22),
        text::HAlign::Center,
        name_color,
        lang,
        text::Size::CardName,
    )?;

    Text::with_alignment(
        hhmm,
        Point::new(x + CARD_W as i32 / 2, CARD_Y + 60),
        time_style,
        Alignment::Center,
    )
    .draw(display)
    .map_err(|_| anyhow::anyhow!("draw error"))?;

    Ok(())
}

/// Draws the minute-cadence dynamic regions: the header line (city/date/
/// weekday/wall clock) and the "next vakit" label. Each region clears only its
/// own small bounding box first instead of the whole panel, which is what made
/// the previous full-screen redraw visibly flicker. The per-second countdown
/// and progress bar are drawn separately by [`draw_countdown`].
fn draw_header<D>(
    display: &mut D,
    local: &LocalTime,
    today: Option<&DayTimes>,
    date_mode: DateMode,
    next_label: &str,
    lang: Language,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Header: city, date (Miladi or Hijri, chosen in settings), weekday, clock.
    let date_part = match (date_mode, today) {
        (DateMode::Hijri, Some(t)) => format!("{} (H)", t.hijri_date),
        _ => format!("{:02}.{:02}.{}", local.day, local.month, local.year),
    };
    Rectangle::new(Point::new(0, 0), Size::new(480, 30))
        .into_styled(PrimitiveStyle::with_fill(col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    if lang.is_rtl() {
        // Keep the numeric city/date/clock left-to-right in the mono font
        // (reversing digits would corrupt them) and render the Arabic weekday
        // on the right, shaped, before the gear icon.
        let latin = format!(
            "HAARLEM   {date_part}   {:02}:{:02}",
            local.hour, local.minute
        );
        Text::new(
            &latin,
            Point::new(10, 20),
            MonoTextStyle::new(&FONT_9X15, col_text()),
        )
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
        let weekday = localize_weekday(local.weekday_name(), lang);
        text::draw_line(
            display,
            weekday,
            Point::new(438, 20),
            text::HAlign::Right,
            col_text(),
            lang,
            text::Size::Small,
        )?;
    } else {
        let weekday = localize_weekday(local.weekday_name(), lang);
        let header_str = format!(
            "HAARLEM   {date_part}   {weekday}   {:02}:{:02}",
            local.hour, local.minute
        );
        text::draw_line(
            display,
            &header_str,
            Point::new(10, 20),
            text::HAlign::Left,
            col_text(),
            lang,
            text::Size::Small,
        )?;
    }
    // Gear icon lives in the header band and must be repainted after the header
    // clears its region each minute.
    settings_screen::draw_gear_icon(display)?;

    // Next-vakit label (localized prefix + localized prayer name).
    let next_line = format!(
        "{} {}",
        language::text(lang, Msg::NextPrayer),
        localize_prayer(next_label, lang)
    );
    Rectangle::new(Point::new(0, 40), Size::new(480, 22))
        .into_styled(PrimitiveStyle::with_fill(col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    text::draw_line(
        display,
        &next_line,
        Point::new(240, 58),
        text::HAlign::Center,
        col_accent(),
        lang,
        text::Size::Medium,
    )?;

    Ok(())
}

/// Draws the per-second dynamic regions: the big seven-segment countdown
/// ("HH:MM:SS") and the progress bar.
///
/// The countdown is rendered into `fb` (a RAM framebuffer) and flushed to the
/// panel in a single SPI transfer. Batching the per-segment rectangle draws
/// behind that buffer is what lets this run every second without the lag or
/// flicker a live clear-then-redraw of the clock box would cause — so unlike
/// the old minute-resolution display, seconds are now shown.
fn draw_countdown<D>(
    display: &mut D,
    fb: &mut FrameBuf,
    remaining_secs: i64,
    progress: Option<f32>,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let total = remaining_secs.max(0);
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    let countdown = format!("{h:02}:{m:02}:{s:02}");

    // Render the seven-segment glyphs into the RAM framebuffer (no SPI traffic),
    // clearing last frame's segments first, then push the whole clock box to the
    // panel in one batched transfer. Drawing is in buffer-local coordinates, so
    // the digits start at x=0 and the buffer's width equals the drawn width.
    fb.clear_fill(col_bg());
    segdisplay::draw_big_time(
        fb,
        Point::new(0, 0),
        CD_DIGIT_W,
        CD_DIGIT_H,
        CD_THICK,
        CD_GAP,
        &countdown,
        col_accent(),
    )
    .map_err(|_| anyhow::anyhow!("draw error"))?;
    let start_x = 240 - (fb.width() as i32) / 2;
    fb.flush(display, Point::new(start_x, CD_DIGITS_Y))
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    // Progress bar (elapsed fraction of the current inter-prayer interval)
    let bar_x = 40;
    let bar_y = 190;
    let bar_w = 400u32;
    let bar_h = 10u32;
    Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w, bar_h))
        .into_styled(PrimitiveStyle::with_fill(col_dim()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    if let Some(p) = progress {
        let filled = (p.clamp(0.0, 1.0) * bar_w as f32) as u32;
        if filled > 0 {
            Rectangle::new(Point::new(bar_x, bar_y), Size::new(filled, bar_h))
                .into_styled(PrimitiveStyle::with_fill(col_accent()))
                .draw(display)
                .map_err(|_| anyhow::anyhow!("draw error"))?;
        }
    }

    Ok(())
}
