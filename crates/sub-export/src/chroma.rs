//! The chroma format an export encodes in (docs/PLAN.md §5.5).
//!
//! The composited frames are RGBA, and every encoder the exporter drives wants
//! a YUV format instead. Which one is not a detail: left unpinned,
//! `videoconvert` hands `x264enc` the least-lossy format it will take, which is
//! `Y444`, and the file comes out in High 4:4:4 Predictive — a profile browsers,
//! phones and every hardware decoder refuse. A delivery file is 4:2:0, so that
//! is what an export pins unless a preset says otherwise.
//!
//! Each variant names the raw formats that carry it, best first. Selection
//! intersects that list with what the chosen encoder's sink pad declares and
//! keeps the *element's* order, so `x264enc` gets `I420` and a VA-API or NVENC
//! element gets the `NV12` it is built around, without either being named here.
//!
//! ```
//! use sub_export::ChromaFormat;
//!
//! assert_eq!(ChromaFormat::default(), ChromaFormat::Yuv420);
//! assert_eq!(ChromaFormat::parse("4:4:4"), Some(ChromaFormat::Yuv444));
//! assert_eq!(
//!     ChromaFormat::Yuv420.declared_in(&["Y444".to_owned(), "NV12".to_owned()]),
//!     vec!["NV12"],
//! );
//! ```

use serde::{Deserialize, Serialize};

/// How the chroma planes are subsampled in the frames handed to the encoder.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ChromaFormat {
    /// 4:2:0, what every delivery profile and every hardware decoder takes.
    #[default]
    Yuv420,
    /// 4:2:2, for a mezzanine an editor will grade.
    Yuv422,
    /// 4:4:4, full chroma: a master, not a delivery file.
    Yuv444,
}

/// Every chroma format, in the order they are reported.
pub const CHROMA_FORMATS: [ChromaFormat; 3] = [
    ChromaFormat::Yuv420,
    ChromaFormat::Yuv422,
    ChromaFormat::Yuv444,
];

impl ChromaFormat {
    /// The stable identifier used in presets, settings and agent calls.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yuv420 => "4:2:0",
            Self::Yuv422 => "4:2:2",
            Self::Yuv444 => "4:4:4",
        }
    }

    /// A human-readable name for the panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Yuv420 => "4:2:0",
            Self::Yuv422 => "4:2:2 (higher chroma)",
            Self::Yuv444 => "4:4:4 (full chroma)",
        }
    }

    /// Parses the stable identifier.
    ///
    /// The colon-free spelling a hurried preset writes — `420` — is accepted
    /// too, because it means exactly one thing.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        CHROMA_FORMATS
            .into_iter()
            .find(|chroma| chroma.as_str() == name || chroma.as_str().replace(':', "") == name)
    }

    /// The `video/x-raw` formats that carry this subsampling, best first.
    ///
    /// The order is only a fallback: when the encoder declares what it takes,
    /// the element's own order wins ([`ChromaFormat::declared_in`]).
    #[must_use]
    pub fn formats(self) -> &'static [&'static str] {
        match self {
            Self::Yuv420 => &["I420", "NV12", "YV12", "NV21"],
            Self::Yuv422 => &["Y42B", "NV16", "NV61", "UYVY", "YUY2", "YVYU"],
            Self::Yuv444 => &["Y444", "NV24", "VUYA", "AYUV"],
        }
    }

    /// The formats of this family that `declared` names, in `declared`'s order.
    ///
    /// `declared` is what an encoder's sink pad says it takes; the result is
    /// empty exactly when this element cannot be fed this chroma format at all,
    /// which is the case the pipeline turns into a named error rather than
    /// letting `videoconvert` negotiate something else.
    #[must_use]
    pub fn declared_in(self, declared: &[String]) -> Vec<&'static str> {
        let mut chosen: Vec<&'static str> = Vec::new();
        for name in declared {
            if let Some(format) = self
                .formats()
                .iter()
                .copied()
                .find(|format| *format == name.as_str())
                && !chosen.contains(&format)
            {
                chosen.push(format);
            }
        }
        chosen
    }
}

impl std::fmt::Display for ChromaFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{CHROMA_FORMATS, ChromaFormat};

    #[test]
    fn the_default_is_the_deliverable_one() {
        assert_eq!(ChromaFormat::default(), ChromaFormat::Yuv420);
    }

    #[test]
    fn identifiers_round_trip_in_both_spellings() {
        for chroma in CHROMA_FORMATS {
            assert_eq!(ChromaFormat::parse(chroma.as_str()), Some(chroma));
            let terse = chroma.as_str().replace(':', "");
            assert_eq!(ChromaFormat::parse(&terse), Some(chroma));
            assert_eq!(ChromaFormat::parse(&format!("  {chroma}  ")), Some(chroma));
        }
        assert_eq!(ChromaFormat::parse("4:1:1"), None);
        assert_eq!(ChromaFormat::parse(""), None);
    }

    #[test]
    fn no_raw_format_belongs_to_two_families() {
        let mut all: Vec<&str> = CHROMA_FORMATS
            .into_iter()
            .flat_map(|chroma| chroma.formats().iter().copied())
            .collect();
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total, "a raw format names one subsampling");
    }

    #[test]
    fn selection_keeps_the_elements_own_order() {
        // What `x264enc` declares, in its own order: 4:4:4 first, which is
        // exactly why an unpinned pipeline ends up there.
        let x264: Vec<String> = ["Y444", "Y42B", "I420", "YV12", "NV12", "GRAY8"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(
            ChromaFormat::Yuv420.declared_in(&x264),
            vec!["I420", "YV12", "NV12"],
        );
        assert_eq!(ChromaFormat::Yuv422.declared_in(&x264), vec!["Y42B"]);
        assert_eq!(ChromaFormat::Yuv444.declared_in(&x264), vec!["Y444"]);
    }

    #[test]
    fn a_hardware_encoder_gets_the_one_format_it_takes() {
        let nv12 = vec!["NV12".to_owned()];
        assert_eq!(ChromaFormat::Yuv420.declared_in(&nv12), vec!["NV12"]);
        assert!(ChromaFormat::Yuv422.declared_in(&nv12).is_empty());
        assert!(ChromaFormat::Yuv444.declared_in(&nv12).is_empty());
    }

    #[test]
    fn a_repeated_format_is_named_once() {
        let repeated = vec!["NV12".to_owned(), "I420".to_owned(), "NV12".to_owned()];
        assert_eq!(
            ChromaFormat::Yuv420.declared_in(&repeated),
            vec!["NV12", "I420"],
        );
    }
}
