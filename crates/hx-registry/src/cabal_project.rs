//! Writing the cabal `repository` stanza so plain cabal builds can consume a
//! registry alongside Hackage.

use crate::client::TufInfo;

/// A cabal `repository` stanza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryStanza {
    /// Repository name (also cabal's cache directory name).
    pub name: String,
    /// Base URL (written with a trailing slash).
    pub url: String,
    /// hackage-security settings; `None` or disabled writes `secure: False`.
    pub tuf: Option<TufInfo>,
}

impl RepositoryStanza {
    /// Render the stanza.
    pub fn render(&self) -> String {
        let mut s = format!(
            "repository {}\n  url: {}/\n",
            self.name,
            self.url.trim_end_matches('/')
        );
        match &self.tuf {
            Some(t) if t.enabled && !t.root_keys.is_empty() => {
                s.push_str("  secure: True\n");
                s.push_str(&format!(
                    "  root-keys: {}\n",
                    t.root_keys.join("\n             ")
                ));
                s.push_str(&format!(
                    "  key-threshold: {}\n",
                    t.key_threshold.unwrap_or(1)
                ));
            }
            _ => s.push_str("  secure: False\n"),
        }
        s
    }
}

/// Insert or replace `repository <name>` in a cabal.project text.
///
/// `existing` is the current file contents (`None` if there is no file); a
/// new file gets `packages: ./*.cabal`, cabal's implicit default, so adding
/// the file does not change which packages are built.
pub fn upsert_repository_stanza(existing: Option<&str>, stanza: &RepositoryStanza) -> String {
    let rendered = stanza.render();
    let Some(text) = existing else {
        return format!("packages: ./*.cabal\n\n{rendered}");
    };
    let header = format!("repository {}", stanza.name);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_end().eq_ignore_ascii_case(&header));
    match start {
        Some(i) => {
            // The stanza ends at the next non-indented, non-blank line.
            let mut end = i + 1;
            while end < lines.len()
                && (lines[end].starts_with([' ', '\t']) || lines[end].trim().is_empty())
            {
                end += 1;
            }
            // Keep blank lines that separate the stanza from what follows.
            while end > i + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            let mut out: Vec<String> = lines[..i].iter().map(|s| s.to_string()).collect();
            out.extend(rendered.trim_end().lines().map(str::to_string));
            out.extend(lines[end..].iter().map(|s| s.to_string()));
            let mut joined = out.join("\n");
            joined.push('\n');
            joined
        }
        None => {
            let mut out = text.trim_end().to_string();
            out.push_str("\n\n");
            out.push_str(&rendered);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secure() -> RepositoryStanza {
        RepositoryStanza {
            name: "registry.arcanist.sh".into(),
            url: "https://registry.arcanist.sh".into(),
            tuf: Some(TufInfo {
                enabled: true,
                root_keys: vec!["aa".into(), "bb".into()],
                key_threshold: Some(2),
            }),
        }
    }

    #[test]
    fn test_render_secure_stanza() {
        assert_eq!(
            secure().render(),
            "repository registry.arcanist.sh\n  url: https://registry.arcanist.sh/\n  secure: True\n  root-keys: aa\n             bb\n  key-threshold: 2\n"
        );
    }

    #[test]
    fn test_render_insecure_when_tuf_disabled() {
        let s = RepositoryStanza {
            tuf: Some(TufInfo::default()),
            ..secure()
        };
        assert!(s.render().contains("secure: False"));
    }

    #[test]
    fn test_upsert_creates_project_file() {
        let out = upsert_repository_stanza(None, &secure());
        assert!(out.starts_with("packages: ./*.cabal\n\nrepository registry.arcanist.sh\n"));
    }

    #[test]
    fn test_upsert_appends_and_replaces_idempotently() {
        let original = "packages: .\n\n-- keep me\nconstraints: base <5\n";
        let once = upsert_repository_stanza(Some(original), &secure());
        assert!(once.starts_with(original.trim_end()));
        assert!(once.ends_with("key-threshold: 2\n"));
        let twice = upsert_repository_stanza(Some(&once), &secure());
        assert_eq!(once, twice);

        let changed = RepositoryStanza {
            url: "http://localhost:8787".into(),
            ..secure()
        };
        let replaced = upsert_repository_stanza(
            Some(&format!("{once}\npackage foo\n  ghc-options: -O2\n")),
            &changed,
        );
        assert_eq!(
            replaced.matches("repository registry.arcanist.sh").count(),
            1
        );
        assert!(replaced.contains("url: http://localhost:8787/"));
        assert!(replaced.contains("\npackage foo\n  ghc-options: -O2\n"));
        assert!(replaced.contains("constraints: base <5"));
    }
}
