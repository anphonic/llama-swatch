//! Which llama-swap versions this monitor has been checked against.

/// Oldest llama-swap release this monitor was developed against.
pub const TESTED_MIN: u32 = 249;
/// Newest llama-swap release this monitor has been checked against.
pub const TESTED_MAX: u32 = 262;

/// Release number from a reported version: `v249`, `249`, `v262-abc`, `v262 (abc1234)`.
/// A leading `v` is optional and anything after the digits must start with a separator
/// (`-`, `+`, `_`, space or `(`), so `1.2.3` style strings are not misread as release 1.
/// `0` is what a llama-swap built without a release tag reports, so it counts as unknown.
pub fn parse_release(reported: &str) -> Option<u32> {
    let s = reported.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, rest) = s.split_at(end);
    if digits.is_empty() || !(rest.is_empty() || rest.starts_with(['-', '+', '_', ' ', '('])) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

/// A note for the dashboard when the server reports a release outside the tested range.
/// Unknown or unparseable versions get no note.
pub fn version_note(reported: &str) -> Option<String> {
    let n = parse_release(reported)?;
    if (TESTED_MIN..=TESTED_MAX).contains(&n) {
        return None;
    }
    Some(format!("Tested with llama-swap v{TESTED_MIN}–v{TESTED_MAX}; this server reports v{n}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reported_formats() {
        let cases: [(&str, Option<u32>); 16] = [
            ("v249", Some(249)),
            ("249", Some(249)),
            ("V262", Some(262)),
            (" v262 ", Some(262)),
            ("v262-abc", Some(262)),
            ("v262-3-gabc1234", Some(262)),
            ("v262+dirty", Some(262)),
            ("v262 (abc1234)", Some(262)),
            ("v1.2.3", None),
            ("262abc", None),
            ("0", None),
            ("v", None),
            ("", None),
            ("unknown", None),
            ("dev-262", None),
            ("v99999999999", None),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_release(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn note_only_outside_tested_range() {
        assert_eq!(version_note("v248").as_deref(), Some("Tested with llama-swap v249–v262; this server reports v248"));
        assert_eq!(version_note("v249"), None);
        assert_eq!(version_note("262"), None);
        assert_eq!(version_note("v263-abc").as_deref(), Some("Tested with llama-swap v249–v262; this server reports v263"));
        assert_eq!(version_note("garbage"), None);
        assert_eq!(version_note("0"), None);
    }
}
