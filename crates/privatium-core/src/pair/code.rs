// Project:  Privatium™  |  File: crates/privatium-core/src/pair/code.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The 16-bit pairing code, its two normative renderings — four glyphs and two
//           words — and the parser that accepts either, or the glyphs' labels
//           (spec/protocol.md §7.2, §7.3, spec/pairing-words.txt).

use std::fmt;

/// One entry of the normative glyph table (`spec/protocol.md §7.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    /// The emoji, stored as the exact bytes the table gives — index 8 and 9 keep their
    /// U+FE0F variation selector, and nothing here normalizes it away.
    pub glyph: &'static str,
    /// The label shown beneath the glyph, everywhere it is rendered.
    pub label: &'static str,
}

/// The sixteen glyphs of `spec/protocol.md §7.3`, in index order. Index order is wire
/// meaning; changing it is a breaking protocol change.
///
/// The two variation selectors are written as escapes so that no editor, formatter or
/// normalizing tool can strip them from the source.
pub static GLYPHS: [Glyph; 16] = [
    Glyph {
        glyph: "\u{1F984}",
        label: "Unicorn",
    },
    Glyph {
        glyph: "\u{1F3A7}",
        label: "Headphones",
    },
    Glyph {
        glyph: "\u{1F355}",
        label: "Pizza",
    },
    Glyph {
        glyph: "\u{1F6F8}",
        label: "UFO",
    },
    Glyph {
        glyph: "\u{1F3B8}",
        label: "Guitar",
    },
    Glyph {
        glyph: "\u{1F344}",
        label: "Mushroom",
    },
    Glyph {
        glyph: "\u{1F48E}",
        label: "Diamond",
    },
    Glyph {
        glyph: "\u{1F98A}",
        label: "Fox",
    },
    Glyph {
        glyph: "\u{26A1}\u{FE0F}",
        label: "Lightning",
    },
    Glyph {
        glyph: "\u{1F336}\u{FE0F}",
        label: "Hot Pepper",
    },
    Glyph {
        glyph: "\u{1F9A9}",
        label: "Flamingo",
    },
    Glyph {
        glyph: "\u{1F3A8}",
        label: "Artist Palette",
    },
    Glyph {
        glyph: "\u{1F34D}",
        label: "Pineapple",
    },
    Glyph {
        glyph: "\u{1F341}",
        label: "Maple Leaf",
    },
    Glyph {
        glyph: "\u{1F3B2}",
        label: "Game Die",
    },
    Glyph {
        glyph: "\u{1F353}",
        label: "Strawberry",
    },
];

/// The spellings accepted for each glyph when a person types its label instead of the
/// emoji: the label with its case, spaces and punctuation removed, and for the two-word
/// labels the one word a person says when reading the pad aloud. Longest first, so that
/// a greedy parse of `mapleleaf` finds one glyph rather than two.
const LABEL_SPELLINGS: [&[&str]; 16] = [
    &["unicorn"],
    &["headphones"],
    &["pizza"],
    &["ufo"],
    &["guitar"],
    &["mushroom"],
    &["diamond"],
    &["fox"],
    &["lightning"],
    &["hotpepper", "pepper"],
    &["flamingo"],
    &["artistpalette", "palette"],
    &["pineapple"],
    &["mapleleaf", "maple", "leaf"],
    &["gamedie", "die"],
    &["strawberry"],
];

/// The 256 pairing words, from `spec/pairing-words.txt`, in the file's order. Index order
/// is wire meaning (`spec/protocol.md §7.2`). Parsed once, when the crate is compiled: a
/// file with the wrong count, a blank line or a non-UTF-8 byte is a build error, never a
/// runtime one.
pub const WORDS: [&str; 256] = parse_words(include_str!("../../../../spec/pairing-words.txt"));

/// Every three-letter prefix in the list is unique (`§7.2`), which is what lets a typed
/// word be abbreviated. Prefixes shorter than this are ambiguous by construction.
const PREFIX: usize = 3;

const fn parse_words(text: &'static str) -> [&'static str; 256] {
    let bytes = text.as_bytes();
    let mut words = [""; 256];
    let mut count = 0;
    let mut start = 0;
    let mut i = 0;
    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            // A final newline ends the file rather than starting a 257th, empty word.
            if i == bytes.len() && i == start {
                break;
            }
            if count == 256 {
                panic!("spec/pairing-words.txt holds more than 256 words");
            }
            let (_, tail) = bytes.split_at(start);
            let (word, _) = tail.split_at(i - start);
            if word.is_empty() {
                panic!("spec/pairing-words.txt has a blank line");
            }
            words[count] = match core::str::from_utf8(word) {
                Ok(word) => word,
                Err(_) => panic!("spec/pairing-words.txt is not UTF-8"),
            };
            count += 1;
            start = i + 1;
        }
        i += 1;
    }
    if count != 256 {
        panic!("spec/pairing-words.txt must hold exactly 256 words");
    }
    words
}

/// Why a typed code could not be read. The input is never echoed: a code is a secret for
/// the length of its window, and an error message is the kind of thing that gets logged.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    /// Nothing was typed.
    #[error("enter the pairing code: the four emoji, their four labels, or the two words")]
    Empty,
    /// Something was typed and it is not a code in any accepted rendering.
    #[error(
        "the pairing code was not recognized: expected the four emoji from the pad, their four labels, or the two words shown on the node"
    )]
    Unrecognized,
}

/// The pairing secret: 16 bits from the CSPRNG (`spec/protocol.md §7.2`), rendered as four
/// glyphs (big-endian nibbles) or two words (big-endian bytes), both meaning this integer.
///
/// `Debug` is hand-written so that a `{:?}` of anything holding a code prints nothing a
/// log could carry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Code(u16);

impl Code {
    /// A fresh code from the platform CSPRNG. Never owner-chosen (`§7.2`).
    #[must_use]
    pub fn random() -> Self {
        use rand::RngExt as _;
        Self(rand::rng().random::<u16>())
    }

    /// The code for a known integer — a test, or a fixture.
    #[must_use]
    pub const fn from_u16(value: u16) -> Self {
        Self(value)
    }

    /// The integer both renderings encode.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// The two bytes of the code, big-endian — the PAKE's input (`§7.4.1`).
    #[must_use]
    pub const fn bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// The emoji rendering: four glyphs, most significant nibble first.
    #[must_use]
    pub fn glyphs(self) -> [&'static Glyph; 4] {
        let v = self.0;
        [
            &GLYPHS[usize::from((v >> 12) & 0xF)],
            &GLYPHS[usize::from((v >> 8) & 0xF)],
            &GLYPHS[usize::from((v >> 4) & 0xF)],
            &GLYPHS[usize::from(v & 0xF)],
        ]
    }

    /// The word rendering: two words, most significant byte first.
    #[must_use]
    pub fn words(self) -> [&'static str; 2] {
        let [high, low] = self.bytes();
        [WORDS[usize::from(high)], WORDS[usize::from(low)]]
    }

    /// Read a code a person typed or pasted, in any accepted rendering (`§7.2`):
    ///
    /// - the four glyphs, with or without spaces, with or without their variation
    ///   selectors — a keyboard that emits ⚡ without U+FE0F is still right;
    /// - the four labels, in any case, separated by anything or nothing — `fox pizza
    ///   lightning die` is what a screen-reader user types from the pad; the one-word
    ///   forms of the two-word labels are accepted too;
    /// - the two words, in any case, with spaces, hyphens and punctuation ignored, and
    ///   each word abbreviated to any prefix of three letters or more, since every
    ///   three-letter prefix in the list is unique.
    ///
    /// Anything else is [`CodeError::Unrecognized`], which names what was expected and
    /// never repeats what was typed.
    pub fn parse(text: &str) -> Result<Self, CodeError> {
        let mut glyphs: Vec<u8> = Vec::with_capacity(4);
        let mut tokens: Vec<String> = Vec::new();
        let mut current = String::new();
        for ch in text.chars() {
            if ch.is_ascii_alphabetic() {
                current.push(ch.to_ascii_lowercase());
                continue;
            }
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            if let Some(index) = GLYPHS.iter().position(|g| g.glyph.starts_with(ch)) {
                glyphs.push(index as u8);
            }
            // Everything else — whitespace, hyphens, punctuation, the variation selector,
            // digits, letters outside ASCII — is a separator.
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        if !glyphs.is_empty() {
            return match (glyphs.as_slice(), tokens.is_empty()) {
                ([a, b, c, d], true) => Ok(Self(
                    (u16::from(*a) << 12)
                        | (u16::from(*b) << 8)
                        | (u16::from(*c) << 4)
                        | u16::from(*d),
                )),
                _ => Err(CodeError::Unrecognized),
            };
        }
        if tokens.is_empty() {
            return Err(CodeError::Empty);
        }
        if let [first, second] = tokens.as_slice()
            && let (Some(high), Some(low)) = (word_index(first), word_index(second))
        {
            return Ok(Self((u16::from(high) << 8) | u16::from(low)));
        }
        let joined: String = tokens.concat();
        if let Some(code) = parse_joined_words(&joined) {
            return Ok(code);
        }
        parse_joined_labels(&joined).ok_or(CodeError::Unrecognized)
    }
}

/// The index of a whole word or of an unambiguous abbreviation of one.
fn word_index(token: &str) -> Option<u8> {
    if token.len() < PREFIX {
        return None;
    }
    let position = WORDS.iter().position(|w| w.starts_with(token))?;
    Some(position as u8)
}

/// Two whole words written with nothing between them.
fn parse_joined_words(joined: &str) -> Option<Code> {
    let mut rest = joined;
    let mut bytes = Vec::with_capacity(2);
    while !rest.is_empty() {
        if bytes.len() == 2 || rest.len() < PREFIX {
            return None;
        }
        let index = WORDS
            .iter()
            .position(|w| rest.starts_with(w) && w.starts_with(&rest[..PREFIX]))?;
        bytes.push(index as u8);
        rest = &rest[WORDS[index].len()..];
    }
    match bytes.as_slice() {
        [high, low] => Some(Code((u16::from(*high) << 8) | u16::from(*low))),
        _ => None,
    }
}

/// Four labels, in any of their accepted spellings, written with nothing between them.
fn parse_joined_labels(joined: &str) -> Option<Code> {
    let mut rest = joined;
    let mut nibbles = Vec::with_capacity(4);
    while !rest.is_empty() {
        if nibbles.len() == 4 {
            return None;
        }
        let (index, spelling) = LABEL_SPELLINGS
            .iter()
            .enumerate()
            .flat_map(|(index, spellings)| spellings.iter().map(move |s| (index, *s)))
            .filter(|(_, s)| rest.starts_with(s))
            .max_by_key(|(_, s)| s.len())?;
        nibbles.push(index as u8);
        rest = &rest[spelling.len()..];
    }
    match nibbles.as_slice() {
        [a, b, c, d] => Some(Code(
            (u16::from(*a) << 12) | (u16::from(*b) << 8) | (u16::from(*c) << 4) | u16::from(*d),
        )),
        _ => None,
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build already proved the count; this pins the shape `§7.2` promises of every
    /// word, so a future edit to the list fails here rather than in a phone's keyboard.
    #[test]
    fn the_word_list_is_lowercase_ascii_four_to_six_letters_and_prefix_unique() {
        let mut prefixes = std::collections::BTreeSet::new();
        for word in WORDS {
            assert!((4..=6).contains(&word.len()), "{word}");
            assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "{word}");
            assert!(prefixes.insert(&word[..PREFIX]), "{word}");
        }
    }

    /// A label spelling that is also a pairing word would make a typed code ambiguous.
    /// `guitar` is both, and is disambiguated by count: two tokens are words, four are
    /// labels. Any other collision is a finding.
    #[test]
    fn label_spellings_do_not_collide_with_words_beyond_the_known_one() {
        for (index, spellings) in LABEL_SPELLINGS.iter().enumerate() {
            for spelling in *spellings {
                let collides = WORDS.contains(spelling);
                assert!(
                    !collides || (*spelling == "guitar" && index == 4),
                    "{spelling}"
                );
            }
        }
    }

    #[test]
    fn the_glyph_table_has_sixteen_distinct_first_codepoints() {
        let firsts: std::collections::BTreeSet<char> = GLYPHS
            .iter()
            .filter_map(|g| g.glyph.chars().next())
            .collect();
        assert_eq!(firsts.len(), 16);
    }
}
