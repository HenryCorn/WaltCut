//! UI strings. The interface is English only: strings in code are the display text. The helpers
//! below keep every label going through one place (and `{name}` templates through [`fmt`]), so a
//! later decision to localize again has one seam, but nothing is looked up or translated today.
//! Command ids, automation ids, preference values, the control channel, the CLI and MCP stay English.

/// A UI string as displayed (English only).
pub fn t(text: &str) -> &str {
    text
}

/// Match a lowercased search query against the displayed label.
pub fn matches_query(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(query)
}

/// Like [`t`], for an English string that needs a disambiguating `context`.
pub fn tr_ctx<'a>(_context: &str, text: &'a str) -> &'a str {
    text
}

/// Fill `{name}` placeholders. Unknown placeholders are left as written.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some((prefix, after)) = rest.split_once('{') {
        out.push_str(prefix);
        let Some((name, tail)) = after.split_once('}') else {
            out.push('{');
            out.push_str(after);
            return out;
        };
        if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::fmt;

    #[test]
    fn placeholders_are_filled_and_unknown_ones_kept() {
        assert_eq!(fmt("{a} of {b} {c}", &[("a", "1"), ("b", "2")]), "1 of 2 {c}");
        assert_eq!(fmt("open {", &[]), "open {");
    }
}
