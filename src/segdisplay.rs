//! Minimal seven-segment style digit renderer, used to draw a large digital-clock
//! looking countdown. embedded-graphics' built-in fonts top out at 10x20px, which
//! reads as tiny text rather than a "clock", so segments drawn as plain filled
//! rectangles scale to whatever size we want instead.

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

// Segment order: a(top), b(top-right), c(bottom-right), d(bottom), e(bottom-left), f(top-left), g(middle)
const DIGIT_SEGMENTS: [[bool; 7]; 10] = [
    [true, true, true, true, true, true, false],    // 0
    [false, true, true, false, false, false, false], // 1
    [true, true, false, true, true, false, true],   // 2
    [true, true, true, true, false, false, true],   // 3
    [false, true, true, false, false, true, true],  // 4
    [true, false, true, true, false, true, true],   // 5
    [true, false, true, true, true, true, true],    // 6
    [true, true, true, false, false, false, false], // 7
    [true, true, true, true, true, true, true],     // 8
    [true, true, true, true, false, true, true],    // 9
];

/// Draws a single digit (0-9) as a seven-segment glyph inside a `w`x`h` box
/// whose top-left corner is `pos`, using `thickness`-px wide segments.
pub fn draw_digit<D>(
    display: &mut D,
    pos: Point,
    w: u32,
    h: u32,
    thickness: u32,
    digit: u8,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let segs = DIGIT_SEGMENTS[digit as usize % 10];
    let t = thickness;
    let half_h = (h - t) / 2;
    let style = PrimitiveStyle::with_fill(color);

    let Point { x, y } = pos;

    // a: top
    if segs[0] {
        Rectangle::new(Point::new(x + t as i32, y), Size::new(w - 2 * t, t))
            .into_styled(style)
            .draw(display)?;
    }
    // f: top-left
    if segs[5] {
        Rectangle::new(Point::new(x, y + t as i32), Size::new(t, half_h))
            .into_styled(style)
            .draw(display)?;
    }
    // b: top-right
    if segs[1] {
        Rectangle::new(Point::new(x + (w - t) as i32, y + t as i32), Size::new(t, half_h))
            .into_styled(style)
            .draw(display)?;
    }
    // g: middle
    if segs[6] {
        Rectangle::new(
            Point::new(x + t as i32, y + half_h as i32),
            Size::new(w - 2 * t, t),
        )
        .into_styled(style)
        .draw(display)?;
    }
    // e: bottom-left
    if segs[4] {
        Rectangle::new(
            Point::new(x, y + (half_h + t) as i32),
            Size::new(t, half_h),
        )
        .into_styled(style)
        .draw(display)?;
    }
    // c: bottom-right
    if segs[2] {
        Rectangle::new(
            Point::new(x + (w - t) as i32, y + (half_h + t) as i32),
            Size::new(t, half_h),
        )
        .into_styled(style)
        .draw(display)?;
    }
    // d: bottom
    if segs[3] {
        Rectangle::new(
            Point::new(x + t as i32, y + (h - t) as i32),
            Size::new(w - 2 * t, t),
        )
        .into_styled(style)
        .draw(display)?;
    }

    Ok(())
}

/// Draws a colon (two small squares) at `pos`, vertically centered in a `h`-tall row.
pub fn draw_colon<D>(
    display: &mut D,
    pos: Point,
    w: u32,
    h: u32,
    color: D::Color,
) -> Result<(), D::Error>
where
    D: DrawTarget,
{
    let style = PrimitiveStyle::with_fill(color);
    let dot_h = w;
    let gap = h / 3;
    Rectangle::new(Point::new(pos.x, pos.y + gap as i32), Size::new(w, dot_h))
        .into_styled(style)
        .draw(display)?;
    Rectangle::new(
        Point::new(pos.x, pos.y + (h - gap - dot_h) as i32),
        Size::new(w, dot_h),
    )
    .into_styled(style)
    .draw(display)?;
    Ok(())
}

/// Draws a string containing only `0-9` and `:` as large seven-segment digits,
/// starting at `pos`. Returns the total width drawn.
pub fn draw_big_time<D>(
    display: &mut D,
    pos: Point,
    digit_w: u32,
    digit_h: u32,
    thickness: u32,
    gap: u32,
    text: &str,
    color: D::Color,
) -> Result<u32, D::Error>
where
    D: DrawTarget,
{
    let colon_w = thickness;
    let mut x = pos.x;
    for ch in text.chars() {
        if let Some(d) = ch.to_digit(10) {
            draw_digit(display, Point::new(x, pos.y), digit_w, digit_h, thickness, d as u8, color)?;
            x += digit_w as i32 + gap as i32;
        } else if ch == ':' {
            draw_colon(display, Point::new(x, pos.y), colon_w, digit_h, color)?;
            x += colon_w as i32 + gap as i32;
        }
    }
    Ok((x - gap as i32 - pos.x).max(0) as u32)
}

/// Computes the total pixel width [`draw_big_time`] would use for `text`,
/// without drawing anything (useful for centering).
pub fn measure_big_time(text: &str, digit_w: u32, thickness: u32, gap: u32) -> u32 {
    let mut w = 0u32;
    let mut any = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            w += digit_w + gap;
            any = true;
        } else if ch == ':' {
            w += thickness + gap;
            any = true;
        }
    }
    if any {
        w - gap
    } else {
        0
    }
}
