// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/// Match an event type against a list of glob-style prefix patterns.
///
/// An empty pattern list matches everything. A pattern ending in `*` matches
/// any event type sharing the literal prefix before the `*` (so `vad.*`
/// requires the `vad.` prefix and does not match `vad_something`). Any other
/// pattern must match exactly.
pub fn matches_glob_filter(patterns: &[String], event_type: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }

    patterns.iter().any(|pattern| {
        pattern
            .strip_suffix('*')
            .map_or_else(|| event_type == pattern, |prefix| event_type.starts_with(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::matches_glob_filter;

    fn patterns(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn empty_matches_all() {
        assert!(matches_glob_filter(&[], "anything"));
        assert!(matches_glob_filter(&[], "vad.speech_start"));
    }

    #[test]
    fn dot_star_requires_literal_dot() {
        let pats = patterns(&["vad.*"]);
        assert!(matches_glob_filter(&pats, "vad.speech_start"));
        assert!(matches_glob_filter(&pats, "vad.speech_end"));
        assert!(!matches_glob_filter(&pats, "vad_something"));
        assert!(!matches_glob_filter(&pats, "stt.result"));
    }

    #[test]
    fn bare_star_matches_any_prefix() {
        let pats = patterns(&["vad*"]);
        assert!(matches_glob_filter(&pats, "vad.speech_start"));
        assert!(matches_glob_filter(&pats, "vad_something"));
        assert!(!matches_glob_filter(&pats, "stt.result"));
    }

    #[test]
    fn exact_match() {
        let pats = patterns(&["stt.result"]);
        assert!(matches_glob_filter(&pats, "stt.result"));
        assert!(!matches_glob_filter(&pats, "stt.result.extra"));
    }

    #[test]
    fn multiple_patterns() {
        let pats = patterns(&["vad.*", "stt.result"]);
        assert!(matches_glob_filter(&pats, "vad.x"));
        assert!(matches_glob_filter(&pats, "stt.result"));
        assert!(!matches_glob_filter(&pats, "other.event"));
    }
}
