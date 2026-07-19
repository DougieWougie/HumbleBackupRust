//! Directory-name derivation: bundle-title parsing and filename sanitizing.

/// Make a string safe to use as a single path segment on Linux.
pub fn sanitize(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c == '\0' || c == '/' { '_' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "Unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Split a Humble bundle title into (publisher, bundle_dir).
///
/// "Humble Tech Book Bundle: Intelligent Agents: Agentic AI and Large
/// Language Models by Apress" -> (Some("Apress"), "Agentic AI and Large
/// Language Models"). Publisher is None when the title has no " by
/// <Publisher>" suffix; caller falls back to the per-book publisher.
pub fn parse_bundle_title(title: &str) -> (Option<String>, String) {
    let tail = match title.rsplit_once(':') {
        Some((_, after)) => after.trim(),
        None => title.trim(),
    };
    if let Some((bundle, publisher)) = tail.rsplit_once(" by ") {
        let bundle = bundle.trim();
        let publisher = publisher.trim();
        if !bundle.is_empty() && !publisher.is_empty() {
            return (Some(publisher.to_string()), bundle.to_string());
        }
    }
    let result = if tail.is_empty() { title.trim() } else { tail };
    (None, result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pattern_publisher_and_bundle() {
        let (pub_, bundle) = parse_bundle_title(
            "Humble Tech Book Bundle: Intelligent Agents: \
             Agentic AI and Large Language Models by Apress",
        );
        assert_eq!(pub_.as_deref(), Some("Apress"));
        assert_eq!(bundle, "Agentic AI and Large Language Models");
    }

    #[test]
    fn no_publisher_suffix() {
        assert_eq!(
            parse_bundle_title("Humble Book Bundle: Cybersecurity 2.0"),
            (None, "Cybersecurity 2.0".to_string())
        );
    }

    #[test]
    fn no_colon_at_all() {
        assert_eq!(
            parse_bundle_title("Some Standalone Purchase"),
            (None, "Some Standalone Purchase".to_string())
        );
    }

    #[test]
    fn by_without_colon() {
        assert_eq!(
            parse_bundle_title("Data Science by O'Reilly"),
            (Some("O'Reilly".to_string()), "Data Science".to_string())
        );
    }

    #[test]
    fn last_by_wins() {
        assert_eq!(
            parse_bundle_title("Web Development by Example by SitePoint"),
            (
                Some("SitePoint".to_string()),
                "Web Development by Example".to_string()
            )
        );
    }

    #[test]
    fn sanitize_replaces_slash() {
        assert_eq!(sanitize("AC/DC: Guide"), "AC_DC: Guide");
    }

    #[test]
    fn sanitize_collapses_whitespace_and_trailing_dots() {
        assert_eq!(sanitize("  Foo   Bar. "), "Foo Bar");
    }

    #[test]
    fn sanitize_never_returns_empty() {
        assert_eq!(sanitize("..."), "Unnamed");
    }
}
