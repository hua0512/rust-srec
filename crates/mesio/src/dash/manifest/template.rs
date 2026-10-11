//! `SegmentTemplate` URL templates (ISO/IEC 23009-1 §5.3.9.4.4).

use super::error::{ManifestError, UnsupportedFeature};
use super::limits::MAX_URL_BYTES;

/// Widths above this are rejected rather than allocating arbitrarily long
/// padding; a `u64` has at most 20 digits.
const MAX_WIDTH: usize = 64;

/// Decimal digits of `u64::MAX`.
const MAX_U64_DIGITS: usize = 20;

/// Which template attribute is compiled; they allow different identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateKind {
    /// `@media`: exactly one of `$Number$` and `$Time$` is required.
    Media,
    /// `@initialization`: `$Number$` and `$Time$` are not allowed.
    Initialization,
}

impl TemplateKind {
    fn attribute(self) -> &'static str {
        match self {
            Self::Media => "SegmentTemplate@media",
            Self::Initialization => "SegmentTemplate@initialization",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Number { width: usize },
    Time { width: usize },
}

/// A template with `$RepresentationID$`, `$Bandwidth$` and `$$` already
/// substituted, leaving only the per-segment identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Template {
    parts: Vec<Part>,
    /// Upper bound on the length of an expansion.
    max_len: usize,
}

impl Template {
    /// Validates `template` and substitutes the per-Representation identifiers.
    ///
    /// Unknown identifiers, malformed format tags and identifiers not allowed
    /// for `kind` are rejected; `$SubNumber$` is reported as unsupported.
    pub(crate) fn compile(
        template: &str,
        kind: TemplateKind,
        representation_id: &str,
        bandwidth: u64,
    ) -> Result<Self, ManifestError> {
        let attribute = kind.attribute();
        let invalid = |reason: &str| ManifestError::InvalidTemplate {
            attribute,
            reason: reason.to_owned(),
        };

        // Every substitution is accounted for before it is written, so a long
        // `$RepresentationID$` repeated many times fails before allocating.
        let mut max_len = 0usize;
        let mut reserve = |additional: usize| -> Result<(), ManifestError> {
            max_len = max_len
                .checked_add(additional)
                .filter(|&len| len <= MAX_URL_BYTES)
                .ok_or(ManifestError::LimitExceeded {
                    what: "expanded URL template",
                    limit: MAX_URL_BYTES,
                })?;
            Ok(())
        };

        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut rest = template;
        let (mut has_number, mut has_time) = (false, false);
        while let Some(start) = rest.find('$') {
            reserve(start)?;
            literal.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('$').ok_or_else(|| invalid("unterminated `$`"))?;
            let token = &after[..end];
            rest = &after[end + 1..];

            if token.is_empty() {
                reserve(1)?;
                literal.push('$');
                continue;
            }
            let (name, format) = match token.split_once('%') {
                Some((name, format)) => (name, Some(format)),
                None => (token, None),
            };
            let width = format
                .map(|format| parse_width(format).ok_or_else(|| invalid("malformed format tag")))
                .transpose()?
                .unwrap_or(0);
            let per_segment = match name {
                "RepresentationID" => {
                    if format.is_some() {
                        return Err(invalid("$RepresentationID$ takes no format tag"));
                    }
                    reserve(representation_id.len())?;
                    literal.push_str(representation_id);
                    None
                }
                "Bandwidth" => {
                    reserve(width.max(MAX_U64_DIGITS))?;
                    push_padded(&mut literal, bandwidth, width);
                    None
                }
                "Number" => {
                    reserve(width.max(MAX_U64_DIGITS))?;
                    has_number = true;
                    Some(Part::Number { width })
                }
                "Time" => {
                    reserve(width.max(MAX_U64_DIGITS))?;
                    has_time = true;
                    Some(Part::Time { width })
                }
                "SubNumber" => {
                    return Err(UnsupportedFeature::TemplateIdentifier(name.to_owned()).into());
                }
                _ => return Err(invalid("unknown identifier")),
            };
            if let Some(part) = per_segment {
                if kind == TemplateKind::Initialization {
                    return Err(invalid("$Number$ and $Time$ are not allowed here"));
                }
                if !literal.is_empty() {
                    parts.push(Part::Literal(std::mem::take(&mut literal)));
                }
                parts.push(part);
            }
        }
        reserve(rest.len())?;
        literal.push_str(rest);
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }

        if kind == TemplateKind::Media {
            match (has_number, has_time) {
                (true, true) => return Err(invalid("$Number$ and $Time$ are mutually exclusive")),
                (false, false) => return Err(invalid("requires $Number$ or $Time$")),
                _ => {}
            }
        }
        Ok(Self { parts, max_len })
    }

    /// Whether segments are addressed by `$Time$` rather than `$Number$`.
    pub(crate) fn uses_time(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, Part::Time { .. }))
    }

    /// Substitutes the segment number and media time (`$Time$` is the raw
    /// media time in timescale units; `@presentationTimeOffset` is not
    /// subtracted).
    pub(crate) fn expand(&self, number: u64, time: u64) -> String {
        let mut out = String::with_capacity(self.max_len);
        for part in &self.parts {
            match *part {
                Part::Literal(ref text) => out.push_str(text),
                Part::Number { width } => push_padded(&mut out, number, width),
                Part::Time { width } => push_padded(&mut out, time, width),
            }
        }
        out
    }
}

/// Appends `value` zero-padded to at least `width` digits (`printf` `%0Nd`).
fn push_padded(out: &mut String, value: u64, width: usize) {
    let digits = value.to_string();
    out.extend(std::iter::repeat_n('0', width.saturating_sub(digits.len())));
    out.push_str(&digits);
}

/// Parses the `0[width]d` part of a `%0[width]d` format tag.
fn parse_width(format: &str) -> Option<usize> {
    let digits = format.strip_prefix('0')?.strip_suffix('d')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&width| width <= MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(template: &str) -> Result<Template, ManifestError> {
        Template::compile(template, TemplateKind::Media, "v1", 500_000)
    }

    #[test]
    fn expands_identifiers() {
        let cases = [
            ("seg_$Number$.m4s", "seg_42.m4s"),
            ("seg_$Number%05d$.m4s", "seg_00042.m4s"),
            ("seg_$Number%01d$.m4s", "seg_42.m4s"),
            ("seg_$Number%010d$.m4s", "seg_0000000042.m4s"),
            ("$RepresentationID$/$Number$.m4s", "v1/42.m4s"),
            ("$Bandwidth$/$Number$.m4s", "500000/42.m4s"),
            ("$Bandwidth%08d$/$Number$.m4s", "00500000/42.m4s"),
            ("cost_$$100_$Number$.m4s", "cost_$100_42.m4s"),
            ("$Number$", "42"),
            ("$$$Number$$$", "$42$"),
        ];
        for (template, expected) in cases {
            assert_eq!(
                media(template).unwrap().expand(42, 84_000),
                expected,
                "{template}"
            );
        }
        assert_eq!(
            media("t/$Time$.m4s").unwrap().expand(1, 900_000_000_000),
            "t/900000000000.m4s"
        );
        assert_eq!(
            media("t/$Time%012d$.m4s").unwrap().expand(1, 84_000),
            "t/000000084000.m4s"
        );
    }

    #[test]
    fn longer_values_are_not_truncated_by_width() {
        assert_eq!(media("$Number%02d$").unwrap().expand(12_345, 0), "12345");
    }

    #[test]
    fn initialization_allows_only_static_identifiers() {
        let init = |t| Template::compile(t, TemplateKind::Initialization, "a 1", 64_000);
        assert_eq!(
            init("$RepresentationID$/init.mp4").unwrap().expand(0, 0),
            "a 1/init.mp4"
        );
        assert_eq!(
            init("init-$Bandwidth$.mp4").unwrap().expand(0, 0),
            "init-64000.mp4"
        );
        assert_eq!(init("init.mp4").unwrap().expand(0, 0), "init.mp4");
        assert!(init("init-$Number$.mp4").is_err());
        assert!(init("init-$Time$.mp4").is_err());
    }

    #[test]
    fn rejects_invalid_templates() {
        for template in [
            "$Unknown$.m4s",
            "$Number.m4s",
            "seg.m4s",
            "$Number$_$Time$.m4s",
            "$Number%5d$.m4s",
            "$Number%0d$.m4s",
            "$Number%05x$.m4s",
            "$Number%0-5d$.m4s",
            "$Number%065d$.m4s",
            "$RepresentationID%05d$/$Number$",
            "$number$.m4s",
        ] {
            assert!(
                matches!(media(template), Err(ManifestError::InvalidTemplate { .. })),
                "{template}"
            );
        }
    }

    #[test]
    fn sub_number_is_unsupported() {
        assert!(matches!(
            media("$Number$_$SubNumber$.m4s"),
            Err(ManifestError::Unsupported(
                UnsupportedFeature::TemplateIdentifier(_)
            ))
        ));
    }

    #[test]
    fn expansion_length_is_bounded_before_allocating() {
        // 4 KiB identifier × 1 000 occurrences would expand to 4 MB.
        let id = "x".repeat(4_096);
        let template = format!("{}$Number$", "$RepresentationID$".repeat(1_000));
        assert!(matches!(
            Template::compile(&template, TemplateKind::Media, &id, 1),
            Err(ManifestError::LimitExceeded { .. })
        ));
        let wide = "$Number%064d$".repeat(1_000);
        assert!(matches!(
            Template::compile(&wide, TemplateKind::Media, "v", 1),
            Err(ManifestError::LimitExceeded { .. })
        ));
        let template =
            Template::compile("$RepresentationID$/$Number$", TemplateKind::Media, &id, 1).unwrap();
        assert!(template.expand(u64::MAX, 0).len() <= template.max_len);
        assert!(!template.uses_time());
        assert!(media("$Time$").unwrap().uses_time());
    }
}
