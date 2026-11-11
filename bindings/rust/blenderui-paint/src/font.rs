//! The faces a view can paint text in. A framework maps each to a font it
//! has; one without faces draws them all regular (the [`PaintSink`]
//! defaults), so measuring and painting still agree.
//!
//! [`PaintSink`]: crate::PaintSink

/// A face of the UI font, or the monospace font.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Font {
    #[default]
    Regular,
    Bold,
    Italic,
    BoldItalic,
    /// The monospace family (CSS `monospace`), regular.
    Mono,
}

impl Font {
    /// Every face, in [`Font::index`] order.
    pub const ALL: [Self; 5] = [Self::Regular, Self::Bold, Self::Italic, Self::BoldItalic, Self::Mono];

    /// The UI font's face for `bold` and `italic`.
    #[must_use]
    pub const fn new(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    /// `0..5`, for tables and cache keys.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    #[must_use]
    pub const fn italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_round_trip() {
        for (i, f) in Font::ALL.iter().enumerate() {
            assert_eq!(f.index(), i);
            if *f != Font::Mono {
                assert_eq!(Font::new(f.bold(), f.italic()), *f);
            }
        }
        assert_eq!(Font::default(), Font::Regular);
    }
}
