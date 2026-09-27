//! Turning a declared licence string into an SPDX expression and judging it
//! against an allowlist.

use spdx::{Expression, ParseMode};
use std::collections::BTreeSet;

/// Common non-SPDX strings seen in package metadata, mapped to SPDX. Anything
/// not here and not parseable fails closed and needs a `clarify` entry.
const MAPPINGS: &[(&str, &str)] = &[
    ("MIT License", "MIT"),
    ("The MIT License", "MIT"),
    ("Apache License 2.0", "Apache-2.0"),
    ("Apache Software License", "Apache-2.0"),
    ("Apache License, Version 2.0", "Apache-2.0"),
    ("BSD License", "BSD-3-Clause"),
    ("3-Clause BSD License", "BSD-3-Clause"),
    ("ISC License (ISCL)", "ISC"),
    ("The Unlicense (Unlicense)", "Unlicense"),
    ("Python Software Foundation License", "PSF-2.0"),
];

/// Parse a declared licence into an SPDX expression, accepting the lax forms
/// packages use in the wild (`MIT/Apache-2.0`, imprecise names).
pub fn parse(declared: &str) -> Option<Expression> {
    let trimmed = declared.trim();
    let mapped = MAPPINGS
        .iter()
        .find(|(from, _)| from.eq_ignore_ascii_case(trimmed))
        .map_or(trimmed, |(_, to)| to);
    Expression::parse_mode(mapped, ParseMode::LAX).ok()
}

/// An allowlist of licence requirements, compared in their SPDX display form
/// (`Apache-2.0 WITH LLVM-exception` is one entry, distinct from `Apache-2.0`).
#[derive(Debug, Clone, Default)]
pub struct Allowlist(BTreeSet<String>);

impl Allowlist {
    pub fn new<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Allowlist(entries.into_iter().map(|e| normalise(e.as_ref())).collect())
    }

    pub fn extend<I, S>(&mut self, entries: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.0
            .extend(entries.into_iter().map(|e| normalise(e.as_ref())));
    }

    pub fn contains(&self, requirement: &str) -> bool {
        self.0.contains(&normalise(requirement))
    }

    /// Whether the expression is satisfiable using only allowed licences:
    /// `OR` needs one allowed branch, `AND` needs every term allowed.
    pub fn permits(&self, expression: &Expression) -> bool {
        expression.evaluate(|req| self.contains(&req.to_string()))
    }
}

/// An allowlist entry in canonical SPDX display form, or an error naming it.
/// Exactly one licence requirement per entry (`Apache-2.0 WITH LLVM-exception`
/// is one); SPDX ids are case-sensitive here, so `apache-2.0` is refused
/// rather than silently never matching.
pub fn validate_entry(entry: &str) -> Result<String, String> {
    let expression = Expression::parse_mode(entry, ParseMode::LAX)
        .map_err(|e| format!("`{entry}` is not an SPDX licence id: {e}"))?;
    let reqs: Vec<_> = expression.requirements().collect();
    match reqs.as_slice() {
        [one] => Ok(one.req.to_string()),
        _ => Err(format!(
            "`{entry}` must be a single licence, not an expression"
        )),
    }
}

/// Canonical form of a requirement for comparison. Entries reaching here are
/// validated at load; anything else is compared verbatim.
fn normalise(entry: &str) -> String {
    validate_entry(entry).unwrap_or_else(|_| entry.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive() -> Allowlist {
        Allowlist::new([
            "MIT",
            "Apache-2.0",
            "Unicode-3.0",
            "Apache-2.0 WITH LLVM-exception",
        ])
    }

    fn permits(allow: &Allowlist, declared: &str) -> bool {
        allow.permits(&parse(declared).expect("parses"))
    }

    #[test]
    fn or_needs_one_allowed_branch() {
        assert!(permits(&permissive(), "MIT OR GPL-3.0"));
        assert!(!permits(&permissive(), "GPL-2.0 OR GPL-3.0"));
    }

    #[test]
    fn and_needs_every_term() {
        // unicode-ident's actual declaration.
        assert!(permits(
            &permissive(),
            "(MIT OR Apache-2.0) AND Unicode-3.0"
        ));
        assert!(!permits(
            &Allowlist::new(["MIT"]),
            "(MIT OR Apache-2.0) AND Unicode-3.0"
        ));
    }

    #[test]
    fn slash_is_or() {
        assert!(permits(&permissive(), "MIT/Apache-2.0"));
    }

    #[test]
    fn with_exception_is_its_own_entry() {
        assert!(permits(&permissive(), "Apache-2.0 WITH LLVM-exception"));
        assert!(!permits(
            &Allowlist::new(["Apache-2.0"]),
            "Apache-2.0 WITH LLVM-exception"
        ));
    }

    #[test]
    fn classifier_names_map_to_spdx() {
        assert!(permits(&permissive(), "MIT License"));
        assert!(permits(&permissive(), "Apache Software License"));
    }

    #[test]
    fn unparseable_fails_closed() {
        assert!(parse("see LICENSE file").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn allowlist_entries_are_validated() {
        assert_eq!(
            validate_entry("Apache-2.0 WITH LLVM-exception").unwrap(),
            "Apache-2.0 WITH LLVM-exception"
        );
        assert!(validate_entry("apache-2.0").is_err(), "case-sensitive");
        assert!(
            validate_entry("MIT OR ISC").is_err(),
            "one licence per entry"
        );
    }
}
