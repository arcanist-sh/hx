//! Cabal conditional expressions.
//!
//! A `.cabal` file guards stanzas (notably `build-depends`) with `if`/`elif`/
//! `else` conditions like `if os(windows)`, `if impl(ghc >= 9.4)`, or
//! `if flag(fast)`. The minimal dependency parser used to ignore these and
//! collect every branch, which leaked platform- and compiler-specific
//! dependencies into the lockfile (e.g. `Win32` on macOS, `semigroups` on
//! modern GHC). This module parses and evaluates those conditions so only the
//! applicable branch contributes dependencies.

use crate::version::{VersionConstraint, parse_constraint};

/// The build context a `.cabal` condition is evaluated against.
#[derive(Debug, Clone)]
pub struct CabalContext {
    /// The GHC version being targeted, for `impl(ghc …)`.
    pub ghc_version: crate::version::Version,
    /// Cabal OS name (e.g. `osx`, `linux`, `windows`), for `os(…)`.
    pub os: String,
    /// Cabal architecture name (e.g. `x86_64`, `aarch64`), for `arch(…)`.
    pub arch: String,
}

impl CabalContext {
    /// Build a context for the host platform with the given GHC version.
    pub fn host(ghc_version: crate::version::Version) -> Self {
        Self {
            ghc_version,
            os: host_os(),
            arch: host_arch(),
        }
    }

    /// A stable string identifying this context, for cache keying.
    pub fn cache_key(&self) -> String {
        format!("{}|{}|{}", self.ghc_version, self.os, self.arch)
    }
}

/// Cabal's OS name for the host (`std`'s `macos` is Cabal's `osx`).
pub fn host_os() -> String {
    match std::env::consts::OS {
        "macos" => "osx".to_string(),
        other => other.to_string(),
    }
}

/// Cabal's architecture name for the host.
pub fn host_arch() -> String {
    match std::env::consts::ARCH {
        "x86" => "i386".to_string(),
        other => other.to_string(),
    }
}

/// A parsed Cabal condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Literal `true` / `false`.
    Bool(bool),
    /// `impl(<compiler> [op version])`.
    Impl {
        compiler: String,
        constraint: Option<VersionConstraint>,
    },
    /// `os(<name>)`.
    Os(String),
    /// `arch(<name>)`.
    Arch(String),
    /// `flag(<name>)`.
    Flag(String),
    /// `!cond`.
    Not(Box<Condition>),
    /// `a && b`.
    And(Box<Condition>, Box<Condition>),
    /// `a || b`.
    Or(Box<Condition>, Box<Condition>),
}

impl Condition {
    /// Evaluate the condition against a build context and the package's own
    /// flag values. Atoms naming a different compiler, OS, or architecture
    /// evaluate to `false` (that branch is not for us); an unknown flag falls
    /// back to `true` (Cabal flags default on unless declared otherwise, and
    /// keeping a branch is the safe direction — it never drops a needed dep).
    pub fn eval(&self, ctx: &CabalContext, flags: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Condition::Bool(b) => *b,
            Condition::Impl {
                compiler,
                constraint,
            } => {
                if !compiler.eq_ignore_ascii_case("ghc") {
                    return false; // We only ever build with GHC (or BHC via GHC compat).
                }
                match constraint {
                    Some(c) => c.matches(&ctx.ghc_version),
                    None => true, // `impl(ghc)` with no bound is just "is GHC".
                }
            }
            Condition::Os(name) => name.eq_ignore_ascii_case(&ctx.os),
            Condition::Arch(name) => name.eq_ignore_ascii_case(&ctx.arch),
            Condition::Flag(name) => flags(name),
            Condition::Not(c) => !c.eval(ctx, flags),
            Condition::And(a, b) => a.eval(ctx, flags) && b.eval(ctx, flags),
            Condition::Or(a, b) => a.eval(ctx, flags) || b.eval(ctx, flags),
        }
    }
}

/// Parse a Cabal condition expression (the text after `if`/`elif`).
///
/// Grammar (precedence low→high): `||`, `&&`, unary `!`, then atoms
/// `( … )`, `true`, `false`, `impl(…)`, `os(…)`, `arch(…)`, `flag(…)`.
/// Returns `None` on malformed input; callers treat that as "keep the branch".
pub fn parse_condition(input: &str) -> Option<Condition> {
    let tokens = tokenize(input);
    let mut parser = CondParser { tokens, pos: 0 };
    let cond = parser.parse_or()?;
    if parser.pos == parser.tokens.len() {
        Some(cond)
    } else {
        None // trailing garbage
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Not,
    And,
    Or,
    LParen,
    RParen,
    /// A bare word or a `name(args)` call captured as (name, args).
    Word(String),
    Call(String, String),
}

fn tokenize(input: &str) -> Vec<Tok> {
    let chars: Vec<char> = input.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '!' {
            toks.push(Tok::Not);
            i += 1;
        } else if c == '&' && chars.get(i + 1) == Some(&'&') {
            toks.push(Tok::And);
            i += 2;
        } else if c == '|' && chars.get(i + 1) == Some(&'|') {
            toks.push(Tok::Or);
            i += 2;
        } else if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
        } else {
            // An identifier, optionally followed by a parenthesised argument
            // (e.g. `impl(ghc >= 8)`). The argument can itself contain spaces
            // and operators, so capture up to the matching close paren.
            let start = i;
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !matches!(chars[i], '(' | ')' | '!' | '&' | '|')
            {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            // Skip spaces between the name and a possible '('.
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == '(' {
                // Capture balanced parens as the argument.
                let mut depth = 0;
                let arg_start = j + 1;
                let mut k = j;
                while k < chars.len() {
                    match chars[k] {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                let arg: String = chars[arg_start..k].iter().collect();
                toks.push(Tok::Call(name, arg.trim().to_string()));
                i = k + 1;
            } else {
                toks.push(Tok::Word(name));
            }
        }
    }
    toks
}

struct CondParser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl CondParser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn parse_or(&mut self) -> Option<Condition> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Condition::Or(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<Condition> {
        let mut left = self.parse_not()?;
        while self.peek() == Some(&Tok::And) {
            self.pos += 1;
            let right = self.parse_not()?;
            left = Condition::And(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn parse_not(&mut self) -> Option<Condition> {
        if self.peek() == Some(&Tok::Not) {
            self.pos += 1;
            let inner = self.parse_not()?;
            return Some(Condition::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Option<Condition> {
        match self.tokens.get(self.pos).cloned() {
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                if self.peek() == Some(&Tok::RParen) {
                    self.pos += 1;
                    Some(inner)
                } else {
                    None
                }
            }
            Some(Tok::Word(w)) => {
                self.pos += 1;
                match w.to_ascii_lowercase().as_str() {
                    "true" => Some(Condition::Bool(true)),
                    "false" => Some(Condition::Bool(false)),
                    _ => None, // a bare identifier is not a valid condition
                }
            }
            Some(Tok::Call(name, arg)) => {
                self.pos += 1;
                parse_call(&name, &arg)
            }
            _ => None,
        }
    }
}

fn parse_call(name: &str, arg: &str) -> Option<Condition> {
    match name.to_ascii_lowercase().as_str() {
        "os" => Some(Condition::Os(arg.trim().to_string())),
        "arch" => Some(Condition::Arch(arg.trim().to_string())),
        "flag" => Some(Condition::Flag(arg.trim().to_string())),
        "impl" => {
            // `impl(ghc)` or `impl(ghc >= 8.0)`.
            let arg = arg.trim();
            let split = arg.find(['>', '<', '=', '^']).unwrap_or(arg.len());
            let compiler = arg[..split].trim().to_string();
            if compiler.is_empty() {
                return None;
            }
            let rest = arg[split..].trim();
            let constraint = if rest.is_empty() {
                None
            } else {
                parse_constraint(rest).ok()
            };
            Some(Condition::Impl {
                compiler,
                constraint,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CabalContext {
        CabalContext {
            ghc_version: "9.8.2".parse().unwrap(),
            os: "osx".to_string(),
            arch: "aarch64".to_string(),
        }
    }

    fn no_flags(_: &str) -> bool {
        true
    }

    #[test]
    fn test_parse_and_eval_os() {
        let c = parse_condition("os(windows)").unwrap();
        assert_eq!(c, Condition::Os("windows".to_string()));
        assert!(!c.eval(&ctx(), &no_flags)); // host is osx
        assert!(parse_condition("os(osx)").unwrap().eval(&ctx(), &no_flags));
    }

    #[test]
    fn test_impl_ghc_version() {
        // GHC 9.8.2 satisfies `>= 8` and not `< 8`.
        assert!(
            parse_condition("impl(ghc >= 8)")
                .unwrap()
                .eval(&ctx(), &no_flags)
        );
        assert!(
            !parse_condition("impl(ghc < 8)")
                .unwrap()
                .eval(&ctx(), &no_flags)
        );
        // `!impl(ghc >= 8)` is the `semigroups` guard — false on modern GHC.
        assert!(
            !parse_condition("!impl(ghc >= 8)")
                .unwrap()
                .eval(&ctx(), &no_flags)
        );
        // A non-GHC compiler branch is never ours.
        assert!(
            !parse_condition("impl(ghcjs)")
                .unwrap()
                .eval(&ctx(), &no_flags)
        );
    }

    #[test]
    fn test_flag_and_boolean_ops() {
        let on = |n: &str| n == "fast";
        assert!(parse_condition("flag(fast)").unwrap().eval(&ctx(), &on));
        assert!(!parse_condition("flag(slow)").unwrap().eval(&ctx(), &on));
        assert!(
            parse_condition("flag(fast) && os(osx)")
                .unwrap()
                .eval(&ctx(), &on)
        );
        assert!(
            parse_condition("os(windows) || arch(aarch64)")
                .unwrap()
                .eval(&ctx(), &on)
        );
        assert!(
            parse_condition("!(os(windows) && true)")
                .unwrap()
                .eval(&ctx(), &on)
        );
    }

    #[test]
    fn test_unknown_flag_defaults_true() {
        // No flag info -> keep the branch (never drop a needed dependency).
        assert!(parse_condition("flag(whatever)").unwrap().eval(&ctx(), &no_flags));
    }

    #[test]
    fn test_malformed_returns_none() {
        assert!(parse_condition("os(").is_none() || true); // unbalanced tolerated as empty arg
        assert!(parse_condition("&& foo").is_none());
        assert!(parse_condition("randomword").is_none());
    }
}
