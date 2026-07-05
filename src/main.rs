mod cache;
mod prayer;
mod segdisplay;
mod time_utils;
mod touch;

use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use embedded_graphics::{
    mono_font::{
        iso_8859_9::{FONT_10X20, FONT_7X13_BOLD, FONT_9X15, FONT_9X18_BOLD},
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

use prayer::DayTimes;
use time_utils::LocalTime;
use touch::Xpt2046;

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

/// Which calendar the header's date is shown in; toggled by tapping the
/// touchscreen anywhere.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DateMode {
    Miladi,
    Hijri,
}

impl DateMode {
    fn toggled(self) -> Self {
        match self {
            DateMode::Miladi => DateMode::Hijri,
            DateMode::Hijri => DateMode::Miladi,
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

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- Backlight (PWM via LEDC, GPIO27) ---
    let ledc_timer = LedcTimerDriver::new(
        peripherals.ledc.timer0,
        &LedcTimerConfig::new().frequency(5.kHz().into()),
    )?;
    let mut backlight = LedcDriver::new(peripherals.ledc.channel0, ledc_timer, peripherals.pins.gpio27)?;
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
    draw_status(&mut display, &["Namaz Vakti", "Başlatılıyor..."])?;

    // --- WiFi ---
    draw_status(&mut display, &["WiFi'ye bağlanılıyor...", CONFIG.wifi_ssid])?;
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))?,
        sys_loop,
    )?;
    if let Err(e) = connect_wifi(&mut wifi) {
        draw_status(&mut display, &["WiFi bağlantısı başarısız", "Yeniden başlatılıyor..."])?;
        log::error!("WiFi connect failed: {e:?}");
        std::thread::sleep(Duration::from_secs(5));
        esp_idf_svc::hal::reset::restart();
    }
    log::info!("WiFi connected");

    // --- Time sync (NTP) ---
    draw_status(&mut display, &["Saat senkronize ediliyor..."])?;
    let sntp = EspSntp::new_default()?;
    let sync_deadline = SystemTime::now() + Duration::from_secs(20);
    while sntp.get_sync_status() != SyncStatus::Completed && SystemTime::now() < sync_deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
    log::info!("SNTP sync status: {:?}", sntp.get_sync_status());

    // --- Prayer time data: try the NVS cache first so a reboot can show the
    // dashboard immediately instead of blocking on a fresh HTTPS fetch ---
    let cache_nvs = cache::open(nvs)?;
    let mut days_data = cache::load(&cache_nvs);
    let mut last_fetch_attempt;
    if days_data.is_empty() {
        draw_status(&mut display, &["Namaz vakitleri", "indiriliyor..."])?;
        days_data = fetch_with_retry(&mut display, 5)?;
        cache::save(&cache_nvs, &days_data);
        last_fetch_attempt = now_epoch();
    } else {
        log::info!("Loaded {} cached prayer-time days from NVS", days_data.len());
        last_fetch_attempt = 0;
    }

    // Tracks what's currently on screen so the main loop only repaints the
    // small regions that actually changed instead of the whole panel (a full
    // 480x320 clear+redraw took 100-200ms and was visibly flickering).
    let mut frame_state: Option<FrameState> = None;
    // The dashboard only shows minute resolution, so it only needs to
    // repaint once a minute rather than every second.
    let mut last_drawn_minute: Option<i64> = None;

    // Tapping the touchscreen toggles the header between Miladi/Hijri dates.
    let mut date_mode = DateMode::Miladi;
    let mut touch_streak = 0u8;
    let mut toggled_this_press = false;

    let mut last_tick = Instant::now() - Duration::from_secs(1); // run the first tick immediately

    // --- Main loop ---
    loop {
        // Touch is polled every iteration (fast) so taps feel responsive;
        // the heavier clock/API-refresh logic below only runs once a second.
        match touch.is_touched() {
            Ok(true) => {
                touch_streak = touch_streak.saturating_add(1);
                if touch_streak == 2 && !toggled_this_press {
                    date_mode = date_mode.toggled();
                    toggled_this_press = true;
                    last_drawn_minute = None; // force header repaint on next tick
                }
            }
            Ok(false) => {
                touch_streak = 0;
                toggled_this_press = false;
            }
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
                draw_status(&mut display, &["Namaz vakti verisi", "eksik, yenileniyor..."])?;
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
            let next_today_label = if next_is_today { Some(next_entry.label) } else { None };

            let day_changed = frame_state
                .as_ref()
                .map(|f| f.today_key != today_key)
                .unwrap_or(true);

            if day_changed {
                draw_static_frame(&mut display, today_row, next_today_label)?;
                last_drawn_minute = None; // force the clock/countdown to repaint too
            } else if frame_state.as_ref().unwrap().next_today_label != next_today_label {
                update_card_highlight(
                    &mut display,
                    today_row,
                    frame_state.as_ref().unwrap().next_today_label,
                    next_today_label,
                )?;
            }

            let current_minute = now_local_secs.div_euclid(60);
            if last_drawn_minute != Some(current_minute) {
                draw_dynamic(
                    &mut display,
                    &local,
                    today_row,
                    date_mode,
                    next_entry.label,
                    remaining,
                    progress,
                )?;
                last_drawn_minute = Some(current_minute);
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

fn fetch_with_retry<D>(display: &mut D, attempts: u32) -> anyhow::Result<Vec<DayTimes>>
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
                    &["Namaz vakitleri indirilemedi", "Tekrar deneniyor..."],
                );
                last_err = Some(e);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("unknown fetch error")))
}

fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> anyhow::Result<()> {
    wifi.set_configuration(&WifiConfiguration::Client(ClientConfiguration {
        ssid: CONFIG.wifi_ssid.try_into().map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: CONFIG.wifi_psk.try_into().map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    Ok(())
}

fn draw_status<D>(display: &mut D, lines: &[&str]) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    display
        .clear(col_bg())
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    let style = MonoTextStyle::new(&FONT_10X20, col_text());
    let mut y = 150 - (lines.len() as i32 - 1) * 12;
    for line in lines {
        Text::with_alignment(line, Point::new(240, y), style, Alignment::Center)
            .draw(display)
            .map_err(|_| anyhow::anyhow!("draw error"))?;
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
            draw_card(display, today, i, next_today_label == Some(name))?;
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
            draw_card(display, today, i, false)?;
        }
        if Some(*name) == new_label {
            draw_card(display, today, i, true)?;
        }
    }
    Ok(())
}

fn draw_card<D>(display: &mut D, today: &DayTimes, index: usize, highlighted: bool) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    let (name, hhmm) = today.prayers()[index];
    let x = CARD_MARGIN + index as i32 * (CARD_W as i32 + CARD_GAP);

    let border_style = PrimitiveStyleBuilder::new()
        .stroke_color(if highlighted { col_accent() } else { col_dim() })
        .stroke_width(1)
        .fill_color(if highlighted { col_accent() } else { col_card_bg() })
        .build();
    Rectangle::new(Point::new(x, CARD_Y), Size::new(CARD_W, CARD_H))
        .into_styled(border_style)
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;

    let (ns, ts) = if highlighted {
        (
            MonoTextStyle::new(&FONT_7X13_BOLD, col_accent_dark()),
            MonoTextStyle::new(&FONT_9X18_BOLD, col_accent_dark()),
        )
    } else {
        (
            MonoTextStyle::new(&FONT_7X13_BOLD, col_dim()),
            MonoTextStyle::new(&FONT_9X18_BOLD, col_text()),
        )
    };

    Text::with_alignment(
        name,
        Point::new(x + CARD_W as i32 / 2, CARD_Y + 22),
        ns,
        Alignment::Center,
    )
    .draw(display)
    .map_err(|_| anyhow::anyhow!("draw error"))?;

    Text::with_alignment(
        hhmm,
        Point::new(x + CARD_W as i32 / 2, CARD_Y + 60),
        ts,
        Alignment::Center,
    )
    .draw(display)
    .map_err(|_| anyhow::anyhow!("draw error"))?;

    Ok(())
}

/// Draws everything that changes over time: the header line (date/weekday/
/// clock), the "next vakit" label, the big countdown, and the progress bar.
/// Called once a minute (the dashboard only shows minute resolution), and
/// each region clears only its own small bounding box first instead of the
/// whole panel, which is what made the previous full-screen-every-second
/// redraw visibly flicker.
#[allow(clippy::too_many_arguments)]
fn draw_dynamic<D>(
    display: &mut D,
    local: &LocalTime,
    today: Option<&DayTimes>,
    date_mode: DateMode,
    next_label: &str,
    remaining_secs: i64,
    progress: Option<f32>,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Header: city, date (Miladi or Hijri — tap the screen to toggle),
    // weekday, clock — a single line.
    let date_part = match (date_mode, today) {
        (DateMode::Hijri, Some(t)) => format!("{} (H)", t.hijri_date),
        _ => format!("{:02}.{:02}.{}", local.day, local.month, local.year),
    };
    let header_str = format!(
        "HAARLEM   {date_part}   {}   {:02}:{:02}",
        local.weekday_name(),
        local.hour,
        local.minute,
    );
    Rectangle::new(Point::new(0, 0), Size::new(480, 30))
        .into_styled(PrimitiveStyle::with_fill(col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    Text::new(
        &header_str,
        Point::new(10, 20),
        MonoTextStyle::new(&FONT_9X15, col_text()),
    )
    .draw(display)
    .map_err(|_| anyhow::anyhow!("draw error"))?;

    // Next-vakit label
    let next_line = format!("SIRADAKİ VAKİT: {next_label}");
    Rectangle::new(Point::new(0, 40), Size::new(480, 22))
        .into_styled(PrimitiveStyle::with_fill(col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    Text::with_alignment(
        &next_line,
        Point::new(240, 58),
        MonoTextStyle::new(&FONT_10X20, col_accent()),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|_| anyhow::anyhow!("draw error"))?;

    // Big countdown, minute resolution ("HH:MM" — seconds were removed since
    // the display only repaints once a minute anyway).
    let h = remaining_secs.max(0) / 3600;
    let m = (remaining_secs.max(0) % 3600) / 60;
    let countdown = format!("{h:02}:{m:02}");
    let (digit_w, digit_h, thickness, gap) = (60u32, 110u32, 14u32, 16u32);
    let width = segdisplay::measure_big_time(&countdown, digit_w, thickness, gap);
    let start_x = 240 - (width as i32) / 2;
    let digits_y = 70i32;
    Rectangle::new(Point::new(start_x, digits_y), Size::new(width, digit_h))
        .into_styled(PrimitiveStyle::with_fill(col_bg()))
        .draw(display)
        .map_err(|_| anyhow::anyhow!("draw error"))?;
    segdisplay::draw_big_time(
        display,
        Point::new(start_x, digits_y),
        digit_w,
        digit_h,
        thickness,
        gap,
        &countdown,
        col_accent(),
    )
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
