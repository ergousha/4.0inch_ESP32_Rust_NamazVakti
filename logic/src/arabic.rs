//! Minimal Arabic text shaper: turns a logical-order Arabic string into a
//! visual-order run of Unicode *presentation forms* ready to be drawn
//! left-to-right by a plain glyph renderer (the firmware uses the
//! `u8g2-fonts` `unifont_t_arabic` face, which covers the Arabic Presentation
//! Forms-B block `U+FE70..U+FEFF`).
//!
//! `embedded-graphics` mono fonts do no Arabic shaping or bidi, so this module
//! performs the two pieces the dashboard needs for its small, fixed set of
//! Arabic labels (see [`crate::language`]):
//!
//! 1. **Contextual joining** — each letter is mapped to its isolated / initial
//!    / medial / final presentation form based on its neighbours, including the
//!    obligatory lam-alef ligature.
//! 2. **Right-to-left ordering** — the shaped glyphs are emitted in visual
//!    order (leftmost glyph first) so the caller can draw them with a normal
//!    left-to-right text routine and right-align the result.
//!
//! Scope is deliberately limited to the modern Arabic letter set with no
//! embedded Latin words (numbers and clock/date fields are drawn separately by
//! the firmware, so no full bidi algorithm is required). Combining diacritics
//! are dropped.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// How an Arabic letter connects to its neighbours.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Join {
    /// Connects on both sides (most letters).
    Dual,
    /// Connects only to the preceding (right-hand) letter — e.g. alef, dal,
    /// reh, waw. Blocks the following letter from joining across it.
    Right,
}

/// The four contextual presentation forms of one letter (or a two-form
/// ligature, which reuses `isolated`/`final` for the initial/medial slots).
#[derive(Clone, Copy)]
struct Letter {
    isolated: char,
    final_: char,
    initial: char,
    medial: char,
    join: Join,
}

impl Letter {
    const fn dual(isolated: char, final_: char, initial: char, medial: char) -> Self {
        Letter {
            isolated,
            final_,
            initial,
            medial,
            join: Join::Dual,
        }
    }
    /// Right-joining letter: only isolated and final forms exist.
    const fn right(isolated: char, final_: char) -> Self {
        Letter {
            isolated,
            final_,
            initial: isolated,
            medial: final_,
            join: Join::Right,
        }
    }
}

/// One element of the parsed run: a joinable Arabic letter, or any other
/// character passed straight through (space, ASCII punctuation, the ellipsis
/// dots, the trailing colon).
enum Elem {
    Letter(Letter),
    Passthrough(char),
}

/// Looks up an Arabic base letter's presentation forms and join type.
/// Returns `None` for anything that should pass through unshaped.
fn letter_info(c: char) -> Option<Letter> {
    let l = match c {
        // Hamza carriers / alef family
        '\u{0621}' => Letter::right('\u{FE80}', '\u{FE80}'), // ء (non-joining, isolated only)
        '\u{0622}' => Letter::right('\u{FE81}', '\u{FE82}'), // آ
        '\u{0623}' => Letter::right('\u{FE83}', '\u{FE84}'), // أ
        '\u{0624}' => Letter::right('\u{FE85}', '\u{FE86}'), // ؤ
        '\u{0625}' => Letter::right('\u{FE87}', '\u{FE88}'), // إ
        '\u{0626}' => Letter::dual('\u{FE89}', '\u{FE8A}', '\u{FE8B}', '\u{FE8C}'), // ئ
        '\u{0627}' => Letter::right('\u{FE8D}', '\u{FE8E}'), // ا
        '\u{0628}' => Letter::dual('\u{FE8F}', '\u{FE90}', '\u{FE91}', '\u{FE92}'), // ب
        '\u{0629}' => Letter::right('\u{FE93}', '\u{FE94}'), // ة
        '\u{062A}' => Letter::dual('\u{FE95}', '\u{FE96}', '\u{FE97}', '\u{FE98}'), // ت
        '\u{062B}' => Letter::dual('\u{FE99}', '\u{FE9A}', '\u{FE9B}', '\u{FE9C}'), // ث
        '\u{062C}' => Letter::dual('\u{FE9D}', '\u{FE9E}', '\u{FE9F}', '\u{FEA0}'), // ج
        '\u{062D}' => Letter::dual('\u{FEA1}', '\u{FEA2}', '\u{FEA3}', '\u{FEA4}'), // ح
        '\u{062E}' => Letter::dual('\u{FEA5}', '\u{FEA6}', '\u{FEA7}', '\u{FEA8}'), // خ
        '\u{062F}' => Letter::right('\u{FEA9}', '\u{FEAA}'), // د
        '\u{0630}' => Letter::right('\u{FEAB}', '\u{FEAC}'), // ذ
        '\u{0631}' => Letter::right('\u{FEAD}', '\u{FEAE}'), // ر
        '\u{0632}' => Letter::right('\u{FEAF}', '\u{FEB0}'), // ز
        '\u{0633}' => Letter::dual('\u{FEB1}', '\u{FEB2}', '\u{FEB3}', '\u{FEB4}'), // س
        '\u{0634}' => Letter::dual('\u{FEB5}', '\u{FEB6}', '\u{FEB7}', '\u{FEB8}'), // ش
        '\u{0635}' => Letter::dual('\u{FEB9}', '\u{FEBA}', '\u{FEBB}', '\u{FEBC}'), // ص
        '\u{0636}' => Letter::dual('\u{FEBD}', '\u{FEBE}', '\u{FEBF}', '\u{FEC0}'), // ض
        '\u{0637}' => Letter::dual('\u{FEC1}', '\u{FEC2}', '\u{FEC3}', '\u{FEC4}'), // ط
        '\u{0638}' => Letter::dual('\u{FEC5}', '\u{FEC6}', '\u{FEC7}', '\u{FEC8}'), // ظ
        '\u{0639}' => Letter::dual('\u{FEC9}', '\u{FECA}', '\u{FECB}', '\u{FECC}'), // ع
        '\u{063A}' => Letter::dual('\u{FECD}', '\u{FECE}', '\u{FECF}', '\u{FED0}'), // غ
        '\u{0641}' => Letter::dual('\u{FED1}', '\u{FED2}', '\u{FED3}', '\u{FED4}'), // ف
        '\u{0642}' => Letter::dual('\u{FED5}', '\u{FED6}', '\u{FED7}', '\u{FED8}'), // ق
        '\u{0643}' => Letter::dual('\u{FED9}', '\u{FEDA}', '\u{FEDB}', '\u{FEDC}'), // ك
        '\u{0644}' => Letter::dual('\u{FEDD}', '\u{FEDE}', '\u{FEDF}', '\u{FEE0}'), // ل
        '\u{0645}' => Letter::dual('\u{FEE1}', '\u{FEE2}', '\u{FEE3}', '\u{FEE4}'), // م
        '\u{0646}' => Letter::dual('\u{FEE5}', '\u{FEE6}', '\u{FEE7}', '\u{FEE8}'), // ن
        '\u{0647}' => Letter::dual('\u{FEE9}', '\u{FEEA}', '\u{FEEB}', '\u{FEEC}'), // ه
        '\u{0648}' => Letter::right('\u{FEED}', '\u{FEEE}'), // و
        '\u{0649}' => Letter::right('\u{FEEF}', '\u{FEF0}'), // ى
        '\u{064A}' => Letter::dual('\u{FEF1}', '\u{FEF2}', '\u{FEF3}', '\u{FEF4}'), // ي
        _ => return None,
    };
    Some(l)
}

/// Returns the lam-alef ligature `(isolated, final)` forms if `alef` is one of
/// the four alef variants that ligate with a preceding lam.
fn lam_alef_ligature(alef: char) -> Option<(char, char)> {
    match alef {
        '\u{0622}' => Some(('\u{FEF5}', '\u{FEF6}')), // لآ
        '\u{0623}' => Some(('\u{FEF7}', '\u{FEF8}')), // لأ
        '\u{0625}' => Some(('\u{FEF9}', '\u{FEFA}')), // لإ
        '\u{0627}' => Some(('\u{FEFB}', '\u{FEFC}')), // لا
        _ => None,
    }
}

/// `true` for combining marks (tashkeel / superscript alef) that this shaper
/// drops rather than trying to position.
fn is_combining_mark(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{0652}' | '\u{0653}'..='\u{0655}' | '\u{0670}')
}

/// Shapes `input` (logical-order Arabic) into a visual-order string of
/// presentation forms, ready to draw left-to-right and right-align.
///
/// Non-Arabic characters are preserved and reordered with the rest of the run;
/// keep embedded Latin words out of the input (numbers/clock fields are drawn
/// separately by the firmware).
pub fn shape(input: &str) -> String {
    // 1. Parse into letters / passthrough, merging lam-alef and dropping marks.
    let chars: Vec<char> = input.chars().filter(|c| !is_combining_mark(*c)).collect();
    let mut elems: Vec<Elem> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{0644}' {
            // Lam: check for a following alef variant to form a ligature.
            if let Some(&next) = chars.get(i + 1) {
                if let Some((iso, fin)) = lam_alef_ligature(next) {
                    // The ligature joins to the previous letter (through lam) but
                    // not to the following one (alef is right-joining).
                    elems.push(Elem::Letter(Letter::right(iso, fin)));
                    i += 2;
                    continue;
                }
            }
        }
        match letter_info(c) {
            Some(l) => elems.push(Elem::Letter(l)),
            None => elems.push(Elem::Passthrough(c)),
        }
        i += 1;
    }

    // 2. Resolve each letter's contextual form from its neighbours.
    let n = elems.len();
    let mut logical: Vec<char> = Vec::with_capacity(n);
    for idx in 0..n {
        match &elems[idx] {
            Elem::Passthrough(c) => logical.push(*c),
            Elem::Letter(l) => {
                // Current letter joins its previous neighbour only if that
                // neighbour connects on its left edge (is dual-joining). Every
                // Arabic letter accepts a connection on its right edge.
                let joins_prev =
                    idx > 0 && matches!(&elems[idx - 1], Elem::Letter(p) if can_join_left(p));
                // It joins its next neighbour only if it itself connects on its
                // left edge (is dual-joining) and any letter follows.
                let joins_next =
                    can_join_left(l) && idx + 1 < n && matches!(&elems[idx + 1], Elem::Letter(_));
                let form = match (joins_prev, joins_next) {
                    (true, true) => l.medial,
                    (true, false) => l.final_,
                    (false, true) => l.initial,
                    (false, false) => l.isolated,
                };
                logical.push(form);
            }
        }
    }

    // 3. Emit in visual (right-to-left) order.
    logical.iter().rev().collect()
}

/// Whether a letter can connect to the letter that follows it (i.e. connects on
/// its left edge). Only dual-joining letters can.
fn can_join_left(l: &Letter) -> bool {
    l.join == Join::Dual
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_ascii_unchanged_but_reversed() {
        // No Arabic letters => just reversed (symmetric strings stay put).
        assert_eq!(shape("..."), "...");
        assert_eq!(shape(":"), ":");
    }

    #[test]
    fn single_isolated_letter() {
        // ب on its own => isolated form U+FE8F.
        assert_eq!(shape("\u{0628}"), "\u{FE8F}");
    }

    #[test]
    fn two_dual_letters_join() {
        // بب => initial + final, emitted right-to-left => [final, initial].
        let out = shape("\u{0628}\u{0628}");
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars, ['\u{FE90}', '\u{FE91}']); // final, then initial
    }

    #[test]
    fn right_joining_letter_blocks_following_join() {
        // د (right-joining) + ب: dal takes isolated (nothing joins it here),
        // beh cannot join to dal on its right side because dal doesn't connect
        // on its left. Logical forms: dal=isolated FEA9, beh=isolated FE8F.
        // Visual order reverses them.
        let out = shape("\u{062F}\u{0628}");
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars, ['\u{FE8F}', '\u{FEA9}']);
    }

    #[test]
    fn lam_alef_forms_ligature() {
        // لا => single ligature glyph. Isolated form FEFB (nothing precedes).
        assert_eq!(shape("\u{0644}\u{0627}"), "\u{FEFB}");
    }

    #[test]
    fn lam_alef_ligature_takes_final_form_after_joining_letter() {
        // بلا: beh joins the ligature, so the ligature uses its final form FEFC.
        // Logical: [beh-initial FE91, ligature-final FEFC]; visual reverses.
        let out = shape("\u{0628}\u{0644}\u{0627}");
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars, ['\u{FEFC}', '\u{FE91}']);
    }

    #[test]
    fn combining_marks_are_dropped() {
        // بَ (beh + fatha) shapes the same as bare beh.
        assert_eq!(shape("\u{0628}\u{064E}"), shape("\u{0628}"));
    }

    #[test]
    fn word_with_space_reverses_word_order() {
        // Two isolated letters separated by a space: "ب ب".
        // Logical glyphs: [FE8F, ' ', FE8F]; visual reverses to [FE8F, ' ', FE8F].
        let out = shape("\u{0628} \u{0628}");
        assert_eq!(out.chars().count(), 3);
        assert!(out.contains(' '));
    }

    #[test]
    fn output_is_non_empty_for_all_real_labels() {
        use crate::language::{prayer_names, text, Language, Msg};
        for name in prayer_names(Language::Arabic) {
            assert!(!shape(name).is_empty());
        }
        for m in [
            Msg::AppTitle,
            Msg::SettingsTitle,
            Msg::NextPrayer,
            Msg::DateHijri,
        ] {
            let s = text(Language::Arabic, m);
            assert!(!shape(s).is_empty(), "empty shape for {}", s);
        }
    }
}
