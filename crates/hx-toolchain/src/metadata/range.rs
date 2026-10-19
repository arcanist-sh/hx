//! Distribution version ranges as written in ghcup metadata.
//!
//! Bindists for a distribution are keyed by a range over the distribution's
//! `VERSION_ID`, e.g. `( >= 10 && < 11 )`, `>= 12`, or `< 10`. The grammar is
//! small: comparisons joined by `&&` and `||`, optionally parenthesized.

use std::cmp::Ordering;

/// Whether `version` falls inside `range`.
///
/// Returns `None` when either side cannot be parsed, so callers can skip the
/// entry instead of guessing.
pub(crate) fn range_matches(range: &str, version: &str) -> Option<bool> {
    let version = parse_version(version)?;
    let tokens = tokenize(range)?;
    let mut parser = Parser { tokens, pos: 0 };
    let result = parser.disjunction(&version)?;
    if parser.pos == parser.tokens.len() {
        Some(result)
    } else {
        None
    }
}

/// Parse a dotted numeric version (`24.04`, `3.20.3`, `12`).
pub(crate) fn parse_version(version: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = version
        .trim()
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect();
    parts.filter(|p| !p.is_empty())
}

/// Compare versions component-wise, treating missing components as zero
/// (`11` == `11.0`, `11.2` > `11`).
pub(crate) fn compare_versions(a: &[u64], b: &[u64]) -> Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Cmp(Op, Vec<u64>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

impl Op {
    fn holds(self, ordering: Ordering) -> bool {
        match self {
            Op::Ge => ordering != Ordering::Less,
            Op::Gt => ordering == Ordering::Greater,
            Op::Le => ordering != Ordering::Greater,
            Op::Lt => ordering == Ordering::Less,
            Op::Eq => ordering == Ordering::Equal,
        }
    }
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let (token, len) = if let Some(r) = rest.strip_prefix('(') {
            (Token::Open, rest.len() - r.len())
        } else if let Some(r) = rest.strip_prefix(')') {
            (Token::Close, rest.len() - r.len())
        } else if rest.starts_with("&&") {
            (Token::And, 2)
        } else if rest.starts_with("||") {
            (Token::Or, 2)
        } else {
            let (op, op_len) = if rest.starts_with(">=") {
                (Op::Ge, 2)
            } else if rest.starts_with("<=") {
                (Op::Le, 2)
            } else if rest.starts_with("==") {
                (Op::Eq, 2)
            } else if rest.starts_with('>') {
                (Op::Gt, 1)
            } else if rest.starts_with('<') {
                (Op::Lt, 1)
            } else {
                return None;
            };
            let after_op = rest[op_len..].trim_start();
            let ver_len = after_op
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(after_op.len());
            let version = parse_version(&after_op[..ver_len])?;
            let consumed = rest.len() - after_op.len() + ver_len;
            (Token::Cmp(op, version), consumed)
        };
        tokens.push(token);
        rest = rest[len..].trim_start();
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn disjunction(&mut self, version: &[u64]) -> Option<bool> {
        let mut result = self.conjunction(version)?;
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            result |= self.conjunction(version)?;
        }
        Some(result)
    }

    fn conjunction(&mut self, version: &[u64]) -> Option<bool> {
        let mut result = self.atom(version)?;
        while self.peek() == Some(&Token::And) {
            self.pos += 1;
            result &= self.atom(version)?;
        }
        Some(result)
    }

    fn atom(&mut self, version: &[u64]) -> Option<bool> {
        match self.tokens.get(self.pos)?.clone() {
            Token::Open => {
                self.pos += 1;
                let inner = self.disjunction(version)?;
                if self.peek() != Some(&Token::Close) {
                    return None;
                }
                self.pos += 1;
                Some(inner)
            }
            Token::Cmp(op, bound) => {
                self.pos += 1;
                Some(op.holds(compare_versions(version, &bound)))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_bounds() {
        assert_eq!(range_matches(">= 12", "12"), Some(true));
        assert_eq!(range_matches(">= 12", "13"), Some(true));
        assert_eq!(range_matches(">= 12", "11"), Some(false));
        assert_eq!(range_matches("< 10", "9"), Some(true));
        assert_eq!(range_matches("< 10", "10"), Some(false));
        assert_eq!(range_matches("== 3.20", "3.20.0"), Some(true));
    }

    #[test]
    fn test_conjunctions_as_written_by_ghcup() {
        assert_eq!(range_matches("( >= 10 && < 11 )", "10"), Some(true));
        assert_eq!(range_matches("(>= 10 && < 11)", "11"), Some(false));
        assert_eq!(range_matches("( >= 22 && < 23 )", "22.04"), Some(true));
        assert_eq!(range_matches("( >= 24 && < 25 )", "24.04"), Some(true));
        assert_eq!(
            range_matches("( >= 3.12 && < 3.18 )", "3.20.3"),
            Some(false)
        );
        assert_eq!(range_matches(">= 3.18", "3.20.3"), Some(true));
        assert_eq!(
            range_matches("( >= 16 && < 18 ) || == 20", "20"),
            Some(true)
        );
    }

    #[test]
    fn test_unparseable() {
        assert_eq!(range_matches("unknown_versioning", "12"), None);
        assert_eq!(range_matches(">= 12", "rolling"), None);
        assert_eq!(range_matches("( >= 10", "10"), None);
        assert_eq!(range_matches(">= 10 11", "10"), None);
    }

    #[test]
    fn test_compare_versions_pads_with_zero() {
        assert_eq!(compare_versions(&[11], &[11, 0]), Ordering::Equal);
        assert_eq!(compare_versions(&[11, 2], &[11]), Ordering::Greater);
        assert_eq!(compare_versions(&[3, 12], &[3, 18]), Ordering::Less);
    }
}
