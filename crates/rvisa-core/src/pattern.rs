//! VISA resource search expressions, as passed to
//! [`Backend::list`](crate::Backend::list).

use crate::error::{Error, Result};
use crate::resource::ResourceName;

/// A compiled VISA search expression.
///
/// The syntax follows VPP-4.3: `?` matches one character, `*` and `+`
/// repeat the preceding item zero-or-more and one-or-more times, `[...]` and
/// `[^...]` are character sets, `|` separates alternatives, `(...)` groups
/// and `\` escapes. Matching is case-insensitive and against the whole
/// canonical name. `"?*"` matches everything.
///
/// ```
/// use rvisa_core::{Pattern, ResourceName};
///
/// let usb = Pattern::new("USB?*INSTR")?;
/// let rn: ResourceName = "USB0::0x1AB1::0x04CE::DS1::INSTR".parse()?;
/// assert!(usb.matches(&rn));
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Pattern {
    regex: regex_lite::Regex,
}

impl Pattern {
    /// Compiles a search expression. Malformed expressions are
    /// [`Error::InvalidArgument`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidArgument`] if the expression is malformed,
    /// such as an unclosed `[` or `(`.
    pub fn new(expr: &str) -> Result<Self> {
        let regex = regex_lite::Regex::new(&translate(expr)).map_err(|e| {
            Error::InvalidArgument(format!("invalid search expression {expr:?}: {e}"))
        })?;
        Ok(Self { regex })
    }

    /// Whether the resource's canonical name matches.
    #[must_use]
    pub fn matches(&self, name: &ResourceName) -> bool {
        self.matches_str(&name.to_string())
    }

    /// Whether `name` matches.
    #[must_use]
    pub fn matches_str(&self, name: &str) -> bool {
        self.regex.is_match(name)
    }
}

/// Converts a VISA expression to an anchored, case-insensitive regex.
fn translate(expr: &str) -> String {
    let mut out = String::with_capacity(expr.len() + 12);
    out.push_str("(?i)^(?:");
    let mut in_set = false;
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(next) => push_literal(&mut out, next),
                None => push_literal(&mut out, '\\'),
            },
            '[' if !in_set => {
                in_set = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    chars.next();
                    out.push('^');
                }
            }
            ']' if in_set => {
                in_set = false;
                out.push(']');
            }
            '?' if !in_set => out.push('.'),
            '*' | '+' | '|' | '(' | ')' if !in_set => out.push(c),
            '-' if in_set => out.push('-'),
            _ => push_literal(&mut out, c),
        }
    }
    out.push_str(")$");
    out
}

fn push_literal(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    out.push_str(&regex_lite::escape(c.encode_utf8(&mut buf)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(expr: &str, name: &str) -> bool {
        Pattern::new(expr).unwrap().matches_str(name)
    }

    #[test]
    fn visa_syntax() {
        assert!(m("?*", "GPIB0::22::INSTR"));
        assert!(m("?*INSTR", "TCPIP0::h::INSTR"));
        assert!(!m("?*INSTR", "TCPIP0::h::5025::SOCKET"));
        assert!(m("GPIB?*", "gpib0::1::instr"));
        assert!(m("GPIB[0-1]::?*", "GPIB1::5::INSTR"));
        assert!(!m("GPIB[^0-1]::?*", "GPIB1::5::INSTR"));
        assert!(m("(GPIB|USB)?*", "USB0::1::2::s::INSTR"));
        assert!(m("ASRL1+::INSTR", "ASRL11::INSTR"));
        assert!(m("ASRL1*::INSTR", "ASRL::INSTR"));
        assert!(!m("GPIB0", "GPIB0::1::INSTR"), "matching is anchored");
    }

    #[test]
    fn regex_metacharacters_are_literal() {
        assert!(m("TCPIP0::10.0.0.1::INSTR", "TCPIP0::10.0.0.1::INSTR"));
        assert!(!m("TCPIP0::10.0.0.1::INSTR", "TCPIP0::10x0x0x1::INSTR"));
        assert!(m("a{2}$^", "A{2}$^"));
        assert!(m(r"\?\*", "?*"));
        assert!(m("[.]", "."));
        assert!(m("trailing\\", "trailing\\"));
    }

    #[test]
    fn malformed_expressions_are_errors() {
        for expr in ["[abc", "(GPIB", "GPIB)", "*abc"] {
            assert!(
                matches!(Pattern::new(expr), Err(Error::InvalidArgument(_))),
                "{expr:?}"
            );
        }
    }

    #[test]
    fn matches_resource_names() {
        let rn: ResourceName = "tcpip::h::5025::socket".parse().unwrap();
        assert!(
            Pattern::new("TCPIP0::h::5025::SOCKET")
                .unwrap()
                .matches(&rn)
        );
    }
}
