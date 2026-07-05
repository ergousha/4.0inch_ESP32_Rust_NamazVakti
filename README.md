# Namaz Vakti — ESP32 Rust Prayer Times Dashboard

A `no_std`-free (std, ESP-IDF-based) Rust firmware for a 4.0" ESP32 TFT board
that connects to WiFi, syncs the clock over NTP, downloads the month's prayer
times for **Haarlem, Netherlands** from the [ezanvakti.emushaf.net](https://ezanvakti.emushaf.net/)
API, and shows a full-screen dashboard: the 5 daily vakits, the currently
upcoming one highlighted, and a live countdown until it. Tap the touchscreen
anywhere to toggle the header's date between Miladi (Gregorian) and Hijri.

This project is the Rust sibling of `../4.0inch_ESP32_LVGL` (the board's
original C/ESP-IDF/LVGL demo) — same board, same display controller, written
from scratch in Rust instead of porting the C code.

## Hardware

Board: **4.0 inch ESP32-32E Display** (also silkscreened as E32R40T /
E32N40T). Product page / vendor wiki with schematics and datasheets:
<https://www.lcdwiki.com/4.0inch_ESP32-32E_Display>

- **MCU**: ESP32 (Xtensa LX6, dual core @ 240MHz), the classic/original
  ESP32, not S2/S3/C-series. Revision v3.1, 4MB flash, **no PSRAM**.
- **Display controller**: ST7796S, driven over SPI (SPI2/HSPI bus),
  320×480 px physically (portrait), RGB565 color, BGR subpixel order.
  This firmware rotates it 90° in software to a 480×320 landscape canvas.
- **Touch controller**: XPT2046 (resistive), shares the same SPI2 bus as the
  display with its own CS line. Used only for tap detection (pressure-based,
  no coordinates) to toggle the header's date mode — see
  [`src/touch.rs`](src/touch.rs).

None of this pinout is silkscreened or in the vendor wiki's text; it was
reverse-engineered from the sibling C project's `sdkconfig`
(`CONFIG_LV_DISP_*` / `CONFIG_LV_TOUCH_*` keys). Keep this table in sync if
you ever target a different board revision.

| Function                | GPIO | Notes |
| ------------------------ | ---- | ----- |
| Display MOSI             | 13   | shared SPI2 bus |
| Display SCLK             | 14   | shared SPI2 bus |
| Display CS               | 15   | |
| Display DC (data/cmd)    | 2    | |
| Display Backlight        | 27   | driven via LEDC PWM (`BACKLIGHT_DUTY_PERCENT` in `main.rs`), not a plain GPIO switch |
| Display Reset            | —    | not wired; tied high, unused in software (`mipidsi`'s `NoResetPin`) |
| Touch (XPT2046) MISO     | 12   | shared SPI2 bus |
| Touch (XPT2046) MOSI/CLK | 13/14| same lines as the display |
| Touch (XPT2046) CS       | 33   | |
| Touch (XPT2046) IRQ      | 36   | not wired up in software — this firmware polls pressure over SPI instead |
| Touch X calibration      | —    | raw ADC range ~110–1971, X inverted (unused — see below) |
| Touch Y calibration      | —    | raw ADC range ~88–1929, Y inverted (unused — see below) |

The X/Y calibration values above are documented for whenever this gets
extended to real touch coordinates; the current firmware only reads the
Z1/Z2 pressure channels to detect "is the screen being tapped", which needs
no calibration. The C driver uses pressure-based touch detection too
(`CONFIG_LV_TOUCH_DETECT_PRESSURE=y`, not the IRQ pin) — this firmware
follows that same proven approach rather than relying on GPIO36 alone,
since GPIO34-39 on the ESP32 have no internal pull resistors.

### Display quirks worth knowing

- **SPI clock: 80MHz, write-only.** Matches the C driver's own config
  (`CONFIG_LV_TFT_SPI_CLK_DIVIDER_1` = undivided 80MHz APB clock). The panel
  handles this fine; don't be surprised it's much faster than the usual
  "safe" 20-40MHz seen in generic SPI TFT tutorials.
- **Mirrored column order.** This specific panel's MADCTL column-address bit
  is the opposite of what the [`mipidsi`](https://docs.rs/mipidsi) crate
  assumes by default, for *any* rotation. It was root-caused by comparing
  against the C driver's known-good MADCTL bytes (`0x48` portrait /
  `0x28` landscape) — both encode the same fixed-up parity. The fix is
  setting `mirrored: true` on `mipidsi::options::Orientation` regardless of
  which `Rotation` you pick. If you ever change the rotation, keep
  `mirrored: true`.
- **BGR, not RGB** subpixel order (`ColorOrder::Bgr`).
- **Touch pressure threshold is untested on real hardware.** `PRESSURE_THRESHOLD`
  in [`src/touch.rs`](src/touch.rs) was picked from the same heuristic the
  popular PJRC/Adafruit `XPT2046_Touchscreen` Arduino driver uses
  (`z1 + (4095 - z2)`), not calibrated against this specific panel. If taps
  are missed or trigger spuriously, that constant is the first thing to tune.

## External services

### Prayer times: ezanvakti.emushaf.net

Turkish Diyanet prayer times API, HTTPS, no API key needed. Endpoints:

| Endpoint | Purpose |
| --- | --- |
| `GET /ulkeler` | list of countries |
| `GET /sehirler/{ulkeId}` | cities/regions in a country |
| `GET /ilceler/{sehirId}` | districts in a city/region |
| `GET /vakitler/{ilceId}` | ~32 days of prayer times for a district |

IDs resolved once for this project and hardcoded in [`src/prayer.rs`](src/prayer.rs)
(`ILCE_ID`): **Hollanda** (Netherlands) → `UlkeID=4` → **Sehir "HOLLANDA"**
(the API lumps all of NL under one pseudo-city) → `SehirID=721` →
**Haarlem** → `IlceID=13877`. To retarget another city, re-run the same
`/ulkeler` → `/sehirler` → `/ilceler` chain and swap `ILCE_ID`.

The firmware caches the fetched month to NVS flash (see "NVS caching of
prayer data" under [Software architecture](#software-architecture) below),
so `/vakitler/13877` is only actually fetched over HTTPS on the very first
boot, or whenever today's
date falls outside the cached ~32-day range (checked every second, throttled
to one attempt per 5 minutes) — in practice about once a month. Of each
day's JSON object, only these fields are used:

- `MiladiTarihKisa` — Gregorian date, `"DD.MM.YYYY"`, used to find "today"'s
  row and shown in the header by default
- `HicriTarihKisa` — Hijri date, `"D.M.YYYY"`, shown in the header instead
  when you tap the touchscreen
- `Imsak`, `Ogle`, `Ikindi`, `Aksam`, `Yatsi` — the 5 vakits, `"HH:MM"`
  (`Gunes`/sunrise is returned too but intentionally not shown — it isn't
  one of the 5 daily prayers)

### Time sync: NTP

`esp_idf_svc::sntp::EspSntp` with ESP-IDF's defaults: `0.pool.ntp.org` through
`3.pool.ntp.org`. The firmware waits up to 20s for a sync at boot before
continuing (it proceeds either way — see [Software architecture](#software-architecture) for
why a failed/slow sync isn't fatal).

**Local time is NOT computed via libc `tzset`/`localtime_r`** — it wasn't
certain those are exposed through every version of the ESP-IDF Rust bindings,
so [`src/time_utils.rs`](src/time_utils.rs) implements Europe/Amsterdam's
CET/CEST offset from scratch (pure calendar math, no dependencies), following
the actual EU DST rule: CEST (UTC+2) from the last Sunday of March 01:00 UTC
to the last Sunday of October 01:00 UTC, CET (UTC+1) otherwise. If you deploy
this outside the EU, that's the function to replace.

## Software architecture

```
src/
├── main.rs        WiFi/SNTP/display/touch/backlight bring-up, main loop, all drawing code
├── prayer.rs       HTTPS fetch + JSON model for the ezanvakti API
├── cache.rs        NVS load/save for the fetched prayer-time month
├── touch.rs        Minimal XPT2046 pressure-only touch driver
├── time_utils.rs   Calendar math + Europe/Amsterdam DST offset (no libc)
└── segdisplay.rs    Seven-segment-style big digit renderer (embedded-graphics
                      primitives), used for the countdown clock
```

- **Rendering**: the panel is only repainted where something actually
  changed, not full-screen every tick — an earlier full `clear()` +
  redraw every second took 100-200ms and was visibly flickering.
  - `draw_static_frame` (whole-panel clear): only on the very first frame and
    on day rollover — draws the header separator and all 5 vakit cards.
  - `update_card_highlight`: redraws just the 1-2 cards whose highlight
    state changed (fires a handful of times a day, whenever the upcoming
    vakit changes).
  - `draw_dynamic`: the header line (date/weekday/clock), the "next vakit"
    label, the big countdown and the progress bar. Runs once a **minute**
    (the dashboard only shows minute resolution) and clears only its own
    small bounding box before redrawing, instead of the whole screen.
- **Fonts**: uses embedded-graphics' `mono_font::iso_8859_9` (Latin-5) fonts,
  not the default `ascii` set — `ascii` only covers 0x20-0x7E and can't
  render Turkish letters (İ, ı, Ş, ş, Ğ, ğ, Ö, ö, Ü, ü, Ç, ç) at all. Vakit
  names, weekday names and status messages use real Turkish spelling
  (İMSAK, ÖĞLE, İKİNDİ, AKŞAM, YATSI, PAZARTESİ, ÇARŞAMBA, PERŞEMBE, ...).
- **Resilience**: if WiFi fails to connect at boot, the device restarts
  itself after showing an error. If the prayer-time fetch fails, it retries
  a few times with a status screen, then keeps retrying in the background
  (throttled to once per 5 minutes) once the main dashboard is showing.
  A failed/slow NTP sync isn't treated as fatal — in practice the ESP-IDF
  SNTP callback finishes shortly after and updates the clock before it's
  actually needed (the HTTPS TLS handshake's certificate-date check already
  requires a correct clock, so by the time prayer data is fetched the time
  is verified good).
- **Touch input**: the display and touch controller share one SPI2 bus
  (`SpiDriver` wrapped in an `Rc`, since `esp-idf-hal`'s `SpiDeviceDriver`
  only needs to *borrow* the bus — each device gets its own hardware CS
  pin and clock speed: 80MHz write-only for the display, 2MHz full-duplex
  for the touch ADC). The main loop polls `Xpt2046::is_touched()` every
  ~120ms; a press is only registered after 2 consecutive positive reads
  (basic debounce), and toggles `DateMode` once per physical tap (tracked
  with a "already toggled this press" flag so holding a finger down
  doesn't rapid-fire).
- **NVS caching of prayer data**: the fetched month is JSON-serialized into
  the `namaz`/`days` NVS blob (see [`src/cache.rs`](src/cache.rs)) every time
  a fetch succeeds. At boot, the cache is tried *before* any network
  activity; if it holds data, the dashboard can render as soon as WiFi+NTP
  are ready without waiting on ezanvakti.emushaf.net at all. The blocking
  "indiriliyor..." fetch screen from a completely empty flash (very first
  boot, or a corrupt/erased NVS) is the only time a boot actually waits on
  the HTTPS call.

## Rust / ESP-IDF toolchain setup

This targets the **classic ESP32**, which is Xtensa (not RISC-V) and needs
Espressif's Rust fork instead of upstream `rustc`, plus a full ESP-IDF
checkout (this uses `esp-idf-hal`/`esp-idf-svc`, i.e. `std`, not the
bare-metal `esp-hal`/`no_std` stack).

One-time setup:

```sh
# Xtensa Rust toolchain + LLVM + GCC
cargo install espup --locked
espup install --targets esp32
. ~/export-esp.sh   # run in every new shell before building; sets PATH/LIBCLANG_PATH

# ESP-IDF's build system needs these (native/system packages, e.g. via
# apt or Homebrew — anything that puts them on PATH works)
#   cmake, ninja

# Linker shim required by esp-idf-hal/esp-idf-svc std builds
cargo install ldproxy --locked

# Flashing tool
cargo install espflash --locked
```

`ESP_IDF_TOOLS_INSTALL_DIR = "workspace"` in [`.cargo/config.toml`](.cargo/config.toml)
means the first `cargo build` clones and builds ESP-IDF itself (~1-2GB,
several minutes) into `.embuild/` inside this project directory — no global
install needed, but expect a slow first build.

## Configuration (WiFi credentials)

WiFi SSID/password are compiled in via [`toml-cfg`](https://github.com/jamesmunns/toml-cfg)
so they never end up hardcoded in a `.rs` file (or in this repo — `cfg.toml`
is gitignored):

```sh
cp cfg.toml.example cfg.toml
# edit cfg.toml with your real SSID/password
```

```toml
# cfg.toml
[namaz-vakti]
wifi_ssid = "YOUR_WIFI_SSID"
wifi_psk = "YOUR_WIFI_PASSWORD"
```

## Build & flash

```sh
. ~/export-esp.sh
cargo build --release
espflash flash --port /dev/ttyUSB0 target/xtensa-esp32-espidf/release/namaz-vakti
espflash monitor --port /dev/ttyUSB0   # optional: view logs over serial
```

### Flashing from WSL2

If you're building on Windows under WSL2, the board enumerates as a Windows
COM port and isn't visible to Linux until you attach it with
[usbipd-win](https://github.com/dorssel/usbipd-win):

```powershell
# Windows PowerShell, as Administrator, one-time:
winget install usbipd

usbipd list                              # find the board's BUSID (look for "CH340"/"CP210x"/"USB Serial")
usbipd bind --busid <BUSID>               # one-time per device
usbipd attach --wsl --busid <BUSID>       # run again any time the board is unplugged/replugged
```

It should then show up in WSL as `/dev/ttyUSB0`.

## Possible future work

- Real touch coordinates (X/Y, using the calibration values in the pinout
  table) for e.g. a settings screen to pick a different city without
  recompiling, instead of the current single "tap anywhere" gesture.
- Actual backlight dimming control (a schedule, an ambient light sensor, or
  just a manually-set day/night level) — the PWM plumbing is already there,
  it's just fixed at `BACKLIGHT_DUTY_PERCENT`.
- Qibla direction (`KibleSaati` is already in the API response, unused).
- Cache eviction/rotation — the NVS blob is simply overwritten on every
  successful fetch, which is fine at this data size (a few KB) but worth
  knowing if you extend what's cached.
