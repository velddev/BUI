//! Wrapping text to a width, once per `(text, width, style)`.
//!
//! Blender wraps a console line on the spot with `textview_wrap_offsets`
//! (`editors/space_info/textview.cc`): one pass over the code points,
//! counting columns, recording the byte offset where each line starts. This
//! is the same pass in pixels, with two additions: the result is cached by
//! the caller ([`crate::measure::TextLayout`]), and ASCII (nearly all chat)
//! is measured with a per-byte advance table filled lazily from the host's
//! text measurement, so a message costs a table lookup per byte instead of
//! a call into the shaper. Other characters are measured one by one by the
//! host (and remembered), an emoji sequence ([`cluster_end`]: a man, a zero
//! width joiner and a bald head are one bald man) as one; the host still
//! shapes the finished lines when it paints. Text in several faces
//! ([`wrap_fonts`]) is measured face by face.

use std::collections::HashMap;

use blenderui_paint::Font;

use crate::measure::FontWidth;

/// Where each line of a wrapped text starts (the first, at 0, is implicit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    starts: Box<[u32]>,
}

impl Lines {
    /// One line (no allocation).
    #[must_use]
    pub fn one() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.starts.len() + 1
    }

    /// Line `i` of `text` without its trailing newline.
    #[must_use]
    pub fn line<'a>(&self, text: &'a str, i: usize) -> &'a str {
        let a = if i == 0 { 0 } else { self.starts[i - 1] as usize };
        let b = self.starts.get(i).map_or(text.len(), |&b| b as usize);
        text[a..b].trim_end_matches(['\n', '\r'])
    }
}

/// Advance widths at one font size, per face: ASCII in a table, the rest in
/// a map.
pub struct Advances {
    size: f32,
    ascii: [[f32; 128]; Font::ALL.len()],
    other: HashMap<(char, Font), f32>,
    seqs: HashMap<(Box<str>, Font), f32>,
}

impl Default for Advances {
    fn default() -> Self {
        Self { size: 0.0, ascii: [[f32::NAN; 128]; Font::ALL.len()], other: HashMap::new(), seqs: HashMap::new() }
    }
}

impl Advances {
    /// Forget everything when the size (or the font) changes.
    pub fn reset(&mut self, size: f32) {
        self.size = size;
        self.ascii = [[f32::NAN; 128]; Font::ALL.len()];
        self.other.clear();
        self.seqs.clear();
    }

    #[must_use]
    pub const fn size(&self) -> f32 {
        self.size
    }

    fn of(&mut self, c: char, font: Font, tw: FontWidth<'_>) -> f32 {
        let mut buf = [0u8; 4];
        let size = self.size;
        let mut measure = |c: char| tw(c.encode_utf8(&mut buf), size, font).max(0.0);
        if c.is_ascii() {
            let slot = &mut self.ascii[font.index()][c as usize];
            if slot.is_nan() {
                *slot = measure(c);
            }
            *slot
        } else {
            *self.other.entry((c, font)).or_insert_with(|| measure(c))
        }
    }

    /// The advance of `s`, one character as it is drawn (an emoji sequence).
    fn of_seq(&mut self, s: &str, font: Font, tw: FontWidth<'_>) -> f32 {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return self.of(c, font, tw);
        }
        let size = self.size;
        *self.seqs.entry((Box::from(s), font)).or_insert_with(|| tw(s, size, font).max(0.0))
    }
}

/// A regional indicator (two make a flag).
const fn is_regional(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

/// Whether `c` joins the character before it (`prev`) into one drawn
/// character: a zero width joiner and what follows one, a variation
/// selector, a skin tone, a keycap, an emoji tag.
const fn joins(prev: char, c: char) -> bool {
    (prev == '\u{200D}' && c != '\n')
        || matches!(c, '\u{200D}' | '\u{20E3}' | '\u{FE00}'..='\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}')
}

/// The end (byte offset) of the character starting at byte `i` of `text` as
/// the text engine draws it: an emoji sequence (U+1F468 U+200D U+1F9B2, a
/// bald man; a skin tone; a keycap; a flag's pair of regional indicators) is
/// one, so it is measured whole and never broken.
#[must_use]
pub fn cluster_end(text: &str, i: usize) -> usize {
    let mut it = text[i..].char_indices();
    let Some((_, first)) = it.next() else { return i };
    let mut end = i + first.len_utf8();
    let mut prev = first;
    for (o, c) in it {
        let pair = is_regional(first) && is_regional(c) && o == first.len_utf8();
        if !pair && !joins(prev, c) {
            break;
        }
        end = i + o + c.len_utf8();
        prev = c;
    }
    end
}

/// Wrap `text` to `max_w` px: break after spaces, at newlines, and inside a
/// word only when the word alone is wider than the line.
pub fn wrap(text: &str, max_w: f32, adv: &mut Advances, tw: &mut dyn FnMut(&str, f32) -> f32) -> Lines {
    wrap_fonts(text, max_w, &[], adv, &mut |s, size, _| tw(s, size))
}

/// [`wrap`] for text in several faces: `fonts` lists where each face starts
/// (byte offset, ascending; the text before the first is regular), and `tw`
/// measures in a face.
pub fn wrap_fonts(text: &str, max_w: f32, fonts: &[(usize, Font)], adv: &mut Advances, tw: FontWidth<'_>) -> Lines {
    let (mut font, mut next) = (Font::Regular, fonts.iter().peekable());
    let mut starts: Vec<u32> = Vec::new();
    let (mut line_w, mut line_start) = (0.0f32, 0usize);
    // the best place to break: byte offset after the last space, and the width up to it
    let mut brk: Option<(usize, f32)> = None;
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        while let Some(&(_, f)) = next.next_if(|(at, _)| *at <= i) {
            font = f;
        }
        // one drawn character: a code point, or an emoji sequence (ASCII only
        // starts one before a keycap's U+FE0F or U+20E3, non-ASCII bytes)
        let end = if text.as_bytes().get(i + 1).is_some_and(|b| *b >= 0x80) || !c.is_ascii() {
            cluster_end(text, i)
        } else {
            i + 1
        };
        let at = i;
        i = end;
        if c == '\n' {
            starts.push((at + 1) as u32);
            (line_w, line_start, brk) = (0.0, at + 1, None);
            continue;
        }
        let w = if end - at > c.len_utf8() { adv.of_seq(&text[at..end], font, tw) } else { adv.of(c, font, tw) };
        if c == ' ' || c == '\t' {
            // trailing blanks may hang past the edge
            line_w += w;
            brk = Some((at + 1, line_w));
            continue;
        }
        if line_w + w > max_w && at > line_start {
            match brk {
                Some((b, w_b)) if b > line_start => {
                    starts.push(b as u32);
                    line_start = b;
                    line_w -= w_b;
                }
                _ => {
                    starts.push(at as u32);
                    line_start = at;
                    line_w = 0.0;
                }
            }
            brk = None;
        }
        line_w += w;
    }
    Lines { starts: starts.into_boxed_slice() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tw(s: &str, size: f32) -> f32 {
        s.chars().count() as f32 * size * 0.5
    }

    fn run(text: &str, w: f32) -> Vec<String> {
        let mut adv = Advances::default();
        adv.reset(10.0);
        let l = wrap(text, w, &mut adv, &mut |s, z| tw(s, z));
        (0..l.count()).map(|i| l.line(text, i).to_owned()).collect()
    }

    #[test]
    fn short_text_is_one_line() {
        assert_eq!(run("hello", 100.0), ["hello"]);
        assert_eq!(run("", 100.0), [""]);
    }

    #[test]
    fn breaks_after_spaces() {
        // 5 px per char: 10 chars per 50 px line
        assert_eq!(run("aaaa bbbb cccc dddd", 50.0), ["aaaa bbbb ", "cccc dddd"]);
    }

    #[test]
    fn long_word_breaks_inside() {
        assert_eq!(run("abcdefghijklmno", 50.0), ["abcdefghij", "klmno"]);
    }

    #[test]
    fn newlines_are_hard_breaks() {
        assert_eq!(run("a\nb\n\nc", 50.0), ["a", "b", "", "c"]);
    }

    #[test]
    fn unicode_is_measured_by_the_host() {
        assert_eq!(run("héllo wörld", 30.0), ["héllo ", "wörld"]);
    }

    #[test]
    fn faces_are_measured_in_their_font() {
        // bold is twice as wide here: "aaaa " regular then "bbbb cccc" bold
        let text = "aaaa bbbb cccc";
        let mut adv = Advances::default();
        adv.reset(10.0);
        let mut tw = |s: &str, z: f32, f: Font| tw(s, z) * if f.bold() { 2.0 } else { 1.0 };
        let l = wrap_fonts(text, 60.0, &[(5, Font::Bold)], &mut adv, &mut tw);
        let lines: Vec<&str> = (0..l.count()).map(|i| l.line(text, i)).collect();
        assert_eq!(lines, ["aaaa ", "bbbb ", "cccc"]);
        // the same text all regular fits on fewer lines
        assert_eq!(wrap_fonts(text, 60.0, &[], &mut adv, &mut tw).count(), 2);
    }

    /// Emoji sequences are one character to the wrap, as they are one glyph
    /// to the text engine: measured whole (once, by the host), never broken.
    #[test]
    fn emoji_sequences_are_measured_whole() {
        let bald = "\u{1F468}\u{200D}\u{1F9B2}";
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let flags = "\u{1F1FA}\u{1F1F8}\u{1F1EB}\u{1F1F7}";
        let keycap = "1\u{FE0F}\u{20E3}";
        let thumbs = "\u{1F44D}\u{1F3FD}";
        let text = format!("{bald}{family}{flags}{keycap}{thumbs}x");
        let mut ends = vec![];
        let mut i = 0;
        while i < text.len() {
            i = cluster_end(&text, i);
            ends.push(i);
        }
        let mut want = vec![];
        let mut at = 0;
        for part in [bald, family, "\u{1F1FA}\u{1F1F8}", "\u{1F1EB}\u{1F1F7}", keycap, thumbs, "x"] {
            at += part.len();
            want.push(at);
        }
        assert_eq!(ends, want);
        // a host that draws a sequence as one 10 px glyph: five fit in 50 px
        let mut seen = vec![];
        let mut tw = |s: &str, _: f32| {
            seen.push(s.to_owned());
            10.0
        };
        let line = format!("{bald}{bald}{bald}{bald}{bald}");
        let mut adv = Advances::default();
        adv.reset(10.0);
        assert_eq!(wrap(&line, 50.0, &mut adv, &mut tw).count(), 1);
        assert_eq!(seen, [bald], "measured once, whole");
        // a long run breaks between sequences, never inside one
        let l = wrap(&format!("{line}{line}"), 50.0, &mut adv, &mut |_, _| 10.0);
        assert_eq!(l.count(), 2);
        assert_eq!(l.line(&format!("{line}{line}"), 1), line);
    }

    #[test]
    fn every_line_but_the_unbreakable_fits() {
        let text = "the quick brown fox jumps over the lazy dog ".repeat(20);
        for l in run(&text, 120.0) {
            assert!(tw(l.trim_end(), 10.0) <= 120.0, "{l:?}");
        }
    }
}
