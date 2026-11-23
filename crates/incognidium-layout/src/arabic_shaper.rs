/// Minimal Arabic-script shaper.
///
/// This is not a full Unicode bidirectional or OpenType shaper. It converts
/// Arabic letters inside a single word into their Unicode presentation forms
/// so that fallback fonts render connected, visually correct glyphs instead of
/// disconnected characters in storage order.
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JoiningType {
    /// Does not join on either side (e.g. hamza, most non-Arabic chars).
    NonJoining,
    /// Joins only with the previous letter (e.g. alef, dal, waw).
    RightJoining,
    /// Joins on both sides (e.g. beh, seen, lam).
    DualJoining,
    /// U+0640 TATWEEL, forces joining on both sides.
    JoinCausing,
}

fn joining_type(ch: char) -> JoiningType {
    match ch as u32 {
        // Hamza and tashkil/diacritics do not join.
        0x0621 => JoiningType::NonJoining,
        // Alef variants only join to the previous letter (exclude waw-hamza and yeh-hamza).
        0x0622..=0x0623 | 0x0625 | 0x0627 | 0x0671 | 0x0672 | 0x0673 | 0x0675 => {
            JoiningType::RightJoining
        }
        // Waw and waw-hamza only join to the previous letter.
        0x0624 | 0x0648 => JoiningType::RightJoining,
        // Yeh with hamza above joins on both sides.
        0x0626 => JoiningType::DualJoining,
        // Beh through khah are dual-joining, except dal/dhal/reh/zain.
        0x0628..=0x0632 => {
            if matches!(ch as u32, 0x062F..=0x0632) {
                JoiningType::RightJoining
            } else {
                JoiningType::DualJoining
            }
        }
        // Seen through ghain are dual-joining.
        0x0633..=0x063A => JoiningType::DualJoining,
        // Tatweel joins on both sides.
        0x0640 => JoiningType::JoinCausing,
        // Feh through yeh are dual-joining.
        0x0641..=0x064A => JoiningType::DualJoining,
        // Persian/Urdu/Dari additions that behave like their Arabic counterparts.
        0x067E | 0x0686 | 0x06A4 | 0x06A9 | 0x06AF | 0x06BE | 0x06C1 | 0x06CC | 0x06CE | 0x06D0
        | 0x06D1 => JoiningType::DualJoining,
        0x06C0 | 0x06D2 | 0x06D3 => JoiningType::RightJoining,
        _ => JoiningType::NonJoining,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Isolated,
    Final,
    Initial,
    Medial,
}

fn form_for_joins(left: bool, right: bool) -> Form {
    match (left, right) {
        (false, false) => Form::Isolated,
        (true, false) => Form::Final,
        (false, true) => Form::Initial,
        (true, true) => Form::Medial,
    }
}

/// Map a base Arabic letter to its four presentation forms.
/// Values are (isolated, final, initial, medial). `None` means the character
/// does not have a presentation-form variant.
fn presentation_forms(ch: char) -> Option<(char, char, char, char)> {
    Some(match ch as u32 {
        0x0621 => ('\u{FE80}', '\u{FE80}', '\u{FE80}', '\u{FE80}'),
        0x0622 => ('\u{FE81}', '\u{FE82}', '\u{FE81}', '\u{FE82}'),
        0x0623 => ('\u{FE83}', '\u{FE84}', '\u{FE83}', '\u{FE84}'),
        0x0624 => ('\u{FE85}', '\u{FE86}', '\u{FE85}', '\u{FE86}'),
        0x0625 => ('\u{FE87}', '\u{FE88}', '\u{FE87}', '\u{FE88}'),
        0x0626 => ('\u{FE89}', '\u{FE8A}', '\u{FE8B}', '\u{FE8C}'),
        0x0627 => ('\u{FE8D}', '\u{FE8E}', '\u{FE8D}', '\u{FE8E}'),
        0x0628 => ('\u{FE8F}', '\u{FE90}', '\u{FE91}', '\u{FE92}'),
        0x0629 => ('\u{FE93}', '\u{FE94}', '\u{FE93}', '\u{FE94}'),
        0x062A => ('\u{FE95}', '\u{FE96}', '\u{FE97}', '\u{FE98}'),
        0x062B => ('\u{FE99}', '\u{FE9A}', '\u{FE9B}', '\u{FE9C}'),
        0x062C => ('\u{FE9D}', '\u{FE9E}', '\u{FE9F}', '\u{FEA0}'),
        0x062D => ('\u{FEA1}', '\u{FEA2}', '\u{FEA3}', '\u{FEA4}'),
        0x062E => ('\u{FEA5}', '\u{FEA6}', '\u{FEA7}', '\u{FEA8}'),
        0x062F => ('\u{FEA9}', '\u{FEAA}', '\u{FEA9}', '\u{FEAA}'),
        0x0630 => ('\u{FEAB}', '\u{FEAC}', '\u{FEAB}', '\u{FEAC}'),
        0x0631 => ('\u{FEAD}', '\u{FEAE}', '\u{FEAD}', '\u{FEAE}'),
        0x0632 => ('\u{FEAF}', '\u{FEB0}', '\u{FEAF}', '\u{FEB0}'),
        0x0633 => ('\u{FEB1}', '\u{FEB2}', '\u{FEB3}', '\u{FEB4}'),
        0x0634 => ('\u{FEB5}', '\u{FEB6}', '\u{FEB7}', '\u{FEB8}'),
        0x0635 => ('\u{FEB9}', '\u{FEBA}', '\u{FEBB}', '\u{FEBC}'),
        0x0636 => ('\u{FEBD}', '\u{FEBE}', '\u{FEBF}', '\u{FEC0}'),
        0x0637 => ('\u{FEC1}', '\u{FEC2}', '\u{FEC3}', '\u{FEC4}'),
        0x0638 => ('\u{FEC5}', '\u{FEC6}', '\u{FEC7}', '\u{FEC8}'),
        0x0639 => ('\u{FEC9}', '\u{FECA}', '\u{FECB}', '\u{FECC}'),
        0x063A => ('\u{FECD}', '\u{FECE}', '\u{FECF}', '\u{FED0}'),
        0x0641 => ('\u{FED1}', '\u{FED2}', '\u{FED3}', '\u{FED4}'),
        0x0642 => ('\u{FED5}', '\u{FED6}', '\u{FED7}', '\u{FED8}'),
        0x0643 => ('\u{FED9}', '\u{FEDA}', '\u{FEDB}', '\u{FEDC}'),
        0x0644 => ('\u{FEDD}', '\u{FEDE}', '\u{FEDF}', '\u{FEE0}'),
        0x0645 => ('\u{FEE1}', '\u{FEE2}', '\u{FEE3}', '\u{FEE4}'),
        0x0646 => ('\u{FEE5}', '\u{FEE6}', '\u{FEE7}', '\u{FEE8}'),
        0x0647 => ('\u{FEE9}', '\u{FEEA}', '\u{FEEB}', '\u{FEEC}'),
        0x0648 => ('\u{FEED}', '\u{FEEE}', '\u{FEED}', '\u{FEEE}'),
        0x0649 => ('\u{FEEF}', '\u{FEF0}', '\u{FEEF}', '\u{FEF0}'),
        0x064A => ('\u{FEF1}', '\u{FEF2}', '\u{FEF3}', '\u{FEF4}'),
        // Persian/Urdu/Dari codepoints.
        0x067E => ('\u{FB56}', '\u{FB57}', '\u{FB58}', '\u{FB59}'),
        0x0686 => ('\u{FB7A}', '\u{FB7B}', '\u{FB7C}', '\u{FB7D}'),
        0x06A4 => ('\u{FB6A}', '\u{FB6B}', '\u{FB6C}', '\u{FB6D}'),
        0x06A9 => ('\u{FB8E}', '\u{FB8F}', '\u{FB90}', '\u{FB91}'),
        0x06AF => ('\u{FB92}', '\u{FB93}', '\u{FB94}', '\u{FB95}'),
        0x06BE => ('\u{FBAA}', '\u{FBAB}', '\u{FBAC}', '\u{FBAD}'),
        0x06C1 => ('\u{FBA0}', '\u{FBA1}', '\u{FBA2}', '\u{FBA3}'),
        0x06CC => ('\u{FBFC}', '\u{FBFD}', '\u{FBFE}', '\u{FBFF}'),
        0x06C0 => ('\u{FBA4}', '\u{FBA5}', '\u{FBA4}', '\u{FBA5}'),
        0x06D2 => ('\u{FBA6}', '\u{FBA7}', '\u{FBA6}', '\u{FBA7}'),
        0x06D3 => ('\u{FBA8}', '\u{FBA9}', '\u{FBA8}', '\u{FBA9}'),
        _ => return None,
    })
}

/// Returns true for characters that benefit from Arabic presentation-form
/// shaping. This covers the basic Arabic block and common Persian/Urdu/Dari
/// extensions.
pub fn is_arabic_script_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE80..=0xFEFF
    ) && presentation_forms(ch).is_some()
}

fn is_lam_alef_pair(lam_form: Form, next: char) -> Option<char> {
    // The pair only forms a ligature when the lam joins to the following alef.
    if !matches!(lam_form, Form::Initial | Form::Medial) {
        return None;
    }
    Some(match next as u32 {
        0x0622 => {
            if lam_form == Form::Medial {
                '\u{FEF6}'
            } else {
                '\u{FEF5}'
            }
        }
        0x0623 => {
            if lam_form == Form::Medial {
                '\u{FEF8}'
            } else {
                '\u{FEF7}'
            }
        }
        0x0625 => {
            if lam_form == Form::Medial {
                '\u{FEFA}'
            } else {
                '\u{FEF9}'
            }
        }
        0x0627 => {
            if lam_form == Form::Medial {
                '\u{FEFC}'
            } else {
                '\u{FEFB}'
            }
        }
        _ => return None,
    })
}

/// Shape a single Arabic word into visual-order presentation-form characters.
///
/// Returns `None` if the word contains no Arabic-script characters that need
/// shaping. The returned vector is ordered left-to-right as it should appear on
/// screen.
pub fn shape_arabic_word_visual(word: &str) -> Option<Vec<char>> {
    let logical: Vec<char> = word.chars().collect();
    if !logical.iter().any(|c| is_arabic_script_char(*c)) {
        return None;
    }

    let types: Vec<JoiningType> = logical.iter().map(|c| joining_type(*c)).collect();
    let mut shaped_logical: Vec<char> = Vec::with_capacity(logical.len());
    let mut prev_joins_to_next = false;

    let mut i = 0;
    while i < logical.len() {
        let cur = logical[i];
        let cur_type = types[i];

        let left_join = prev_joins_to_next
            && matches!(
                cur_type,
                JoiningType::DualJoining | JoiningType::RightJoining | JoiningType::JoinCausing
            );
        let right_join = i + 1 < logical.len()
            && matches!(
                cur_type,
                JoiningType::DualJoining | JoiningType::JoinCausing
            )
            && matches!(
                types[i + 1],
                JoiningType::DualJoining | JoiningType::RightJoining | JoiningType::JoinCausing
            );

        let form = form_for_joins(left_join, right_join);

        // Replace LAM + ALEF variants with their ligature presentation forms.
        // This is the only correct lam-alef direction: lam followed by alef.
        if cur as u32 == 0x0644 && right_join {
            if let Some(lig) = is_lam_alef_pair(form, logical[i + 1]) {
                shaped_logical.push(lig);
                // A lam-alef ligature does not connect to the following letter.
                prev_joins_to_next = false;
                i += 2;
                continue;
            }
        }

        if let Some((iso, fin, init, med)) = presentation_forms(cur) {
            shaped_logical.push(match form {
                Form::Isolated => iso,
                Form::Final => fin,
                Form::Initial => init,
                Form::Medial => med,
            });
        } else {
            shaped_logical.push(cur);
        }
        prev_joins_to_next = right_join;
        i += 1;
    }

    // Reverse to visual order: the first character on the screen is the last
    // shaped logical form.
    shaped_logical.reverse();
    Some(shaped_logical)
}

/// Return a font suitable for rendering shaped Arabic text.
pub fn arabic_fallback_font(bold: bool) -> Option<&'static fontdue::Font> {
    static REGULAR: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    static BOLD: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    let slot = if bold { &BOLD } else { &REGULAR };
    slot.get_or_init(|| {
        for name in ["NotoNaskhArabic", "NotoSansArabic"] {
            let path = format!(
                "/usr/share/fonts/truetype/noto/{}-{}.ttf",
                name,
                if bold { "Bold" } else { "Regular" }
            );
            if let Ok(data) = std::fs::read(&path) {
                if let Ok(font) = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
                {
                    return Some(font);
                }
            }
        }
        None
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shape_alnitaq() {
        // "النطاق" -> visual order should start with an isolated qaf and end
        // with the initial lam and isolated alef of the definite article.
        let visual = shape_arabic_word_visual("النطاق").unwrap();
        // Visual order: first char on screen is the rightmost qaf, then final
        // alef, medial tah, noon, initial lam, isolated alef.
        assert_eq!(
            visual,
            vec!['\u{FED5}', '\u{FE8E}', '\u{FEC4}', '\u{FEE8}', '\u{FEDF}', '\u{FE8D}']
        );
    }

    #[test]
    fn test_latin_no_shaping() {
        assert!(shape_arabic_word_visual("Domain:").is_none());
    }
}
