//! Language-aware text drawing.
//!
//! Latin scripts (Turkish, English) keep using the `embedded-graphics`
//! `iso_8859_9` mono fonts already in use across the dashboard, so İ/ı/Ş/ğ/Ö/Ü/Ç
//! keep rendering. Arabic is drawn through the `u8g2-fonts` renderer with a
//! font that covers the Arabic Presentation Forms-B block, after running the
//! string through [`namaz_vakti_logic::arabic::shape`] (contextual joining +
//! right-to-left ordering). Callers therefore never have to know which script a
//! label is in — they pass the localized string and the active [`Language`].

use embedded_graphics::{
    mono_font::{
        iso_8859_9::{FONT_10X20, FONT_7X13_BOLD, FONT_9X15},
        MonoFont, MonoTextStyle,
    },
    prelude::*,
    text::{Alignment, Text},
};
use u8g2_fonts::{
    fonts,
    types::{FontColor, HorizontalAlignment, VerticalPosition},
    FontRenderer,
};

use namaz_vakti_logic::{arabic, language::Language};

use crate::Rgb565;

/// Text size, chosen to line up the Arabic faces with the existing mono fonts.
#[derive(Clone, Copy)]
pub enum Size {
    /// ~15-16px — header line and secondary labels.
    Small,
    /// ~20px — status/splash lines, the next-prayer label, settings options.
    Medium,
    /// Bold ~13px — the vakit-card name row.
    CardName,
}

/// Horizontal anchoring of the text around the supplied point.
#[derive(Clone, Copy)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

// Arabic renderers. `with_ignore_unknown_chars(true)` keeps a stray ASCII
// separator (a colon, an ellipsis) from turning into a hard render error on a
// face that only carries Arabic glyphs.
const AR_SMALL: FontRenderer =
    FontRenderer::new::<fonts::u8g2_font_unifont_t_arabic>().with_ignore_unknown_chars(true);
const AR_MEDIUM: FontRenderer =
    FontRenderer::new::<fonts::u8g2_font_10x20_t_arabic>().with_ignore_unknown_chars(true);
const AR_CARD: FontRenderer =
    FontRenderer::new::<fonts::u8g2_font_cu12_t_arabic>().with_ignore_unknown_chars(true);

fn mono_font(size: Size) -> &'static MonoFont<'static> {
    match size {
        Size::Small => &FONT_9X15,
        Size::Medium => &FONT_10X20,
        Size::CardName => &FONT_7X13_BOLD,
    }
}

fn arabic_font(size: Size) -> &'static FontRenderer {
    match size {
        Size::Small => &AR_SMALL,
        Size::Medium => &AR_MEDIUM,
        Size::CardName => &AR_CARD,
    }
}

/// Draws a single line of localized text, anchored at `anchor` with its
/// baseline on `anchor.y`. Arabic is shaped and laid out right-to-left; Latin
/// uses the matching mono font.
pub fn draw_line<D>(
    display: &mut D,
    text: &str,
    anchor: Point,
    align: HAlign,
    color: Rgb565,
    lang: Language,
    size: Size,
) -> anyhow::Result<()>
where
    D: DrawTarget<Color = Rgb565>,
{
    if lang.is_rtl() {
        let shaped = arabic::shape(text);
        let ha = match align {
            HAlign::Left => HorizontalAlignment::Left,
            HAlign::Center => HorizontalAlignment::Center,
            HAlign::Right => HorizontalAlignment::Right,
        };
        arabic_font(size)
            .render_aligned(
                shaped.as_str(),
                anchor,
                VerticalPosition::Baseline,
                ha,
                FontColor::Transparent(color),
                display,
            )
            .map_err(|_| anyhow::anyhow!("draw error"))?;
    } else {
        let a = match align {
            HAlign::Left => Alignment::Left,
            HAlign::Center => Alignment::Center,
            HAlign::Right => Alignment::Right,
        };
        Text::with_alignment(text, anchor, MonoTextStyle::new(mono_font(size), color), a)
            .draw(display)
            .map_err(|_| anyhow::anyhow!("draw error"))?;
    }
    Ok(())
}
