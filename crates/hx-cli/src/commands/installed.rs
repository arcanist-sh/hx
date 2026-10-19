//! The manifest of executables installed by `hx install`.
//!
//! `~/.hx/installed.toml` records, per package, exactly which files hx put in
//! `~/.hx/bin`. That is what lets `hx uninstall` remove only what it installed,
//! lets `hx install` recognise an up-to-date install, and lets it refuse to
//! overwrite an executable another package (or the user) owns.

use chrono::{DateTime, Utc};
use hx_core::{Error, Fix, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where an installed package's source came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InstallSource {
    /// A released version from Hackage.
    Hackage,
    /// A git repository; `rev` is the commit that was built.
    Git { url: String, rev: Option<String> },
    /// A local project directory.
    Path { path: PathBuf },
}

impl InstallSource {
    /// Short human-readable description, e.g. `git https://… @ 1a2b3c4`.
    pub fn describe(&self) -> String {
        match self {
            InstallSource::Hackage => "hackage".to_string(),
            InstallSource::Git {
                url,
                rev: Some(rev),
            } => {
                format!("git {url} @ {}", rev.chars().take(12).collect::<String>())
            }
            InstallSource::Git { url, rev: None } => format!("git {url}"),
            InstallSource::Path { path } => format!("path {}", path.display()),
        }
    }
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledPackage {
    /// Package version that was built.
    pub version: String,
    /// File names hx placed in the bin directory for this package.
    pub executables: Vec<String>,
    /// GHC version the package was built with, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ghc: Option<String>,
    /// Where the source came from.
    pub source: InstallSource,
    /// When the install finished.
    pub installed_at: DateTime<Utc>,
}

/// Who already owns an executable name that an install wants to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// Another package installed by `hx install`.
    Package(String),
    /// A file hx did not install (e.g. the hx-managed `ghc` symlink, or
    /// something the user put there).
    Unmanaged,
}

/// An executable name that is already taken in the bin directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub executable: String,
    pub owner: Owner,
}

/// The on-disk manifest (`installed.toml`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstalledManifest {
    /// Installed packages keyed by package name.
    #[serde(default)]
    pub packages: BTreeMap<String, InstalledPackage>,
}

impl InstalledManifest {
    /// Load the manifest; a missing file is an empty manifest.
    pub fn load(path: &Path) -> Result<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(Error::Io {
                    message: "failed to read the installed-packages manifest".to_string(),
                    path: Some(path.to_path_buf()),
                    source: e,
                });
            }
        };
        toml::from_str(&content).map_err(|e| Error::Config {
            message: format!("installed-packages manifest is corrupt: {e}"),
            path: Some(path.to_path_buf()),
            source: None,
            fixes: vec![Fix::new(format!(
                "Repair or delete {} (deleting it forgets which executables in the \
                 bin directory hx installed)",
                path.display()
            ))],
        })
    }

    /// Write the manifest atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| Error::config(format!("failed to serialize installed manifest: {e}")))?;
        let io_err = |e| Error::Io {
            message: "failed to write the installed-packages manifest".to_string(),
            path: Some(path.to_path_buf()),
            source: e,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        hx_core::atomic_write(path, content).map_err(io_err)
    }

    /// The package that installed `executable`, if any.
    pub fn owner_of(&self, executable: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|(_, pkg)| pkg.executables.iter().any(|e| e == executable))
            .map(|(name, _)| name.as_str())
    }

    /// Executables `package` wants to install that something else already
    /// occupies in `bin_dir`. Names the package itself installed earlier are
    /// not collisions: reinstalling replaces them.
    pub fn collisions(
        &self,
        package: &str,
        executables: &[String],
        bin_dir: &Path,
    ) -> Vec<Collision> {
        executables
            .iter()
            .filter_map(|exe| {
                let owner = match self.owner_of(exe) {
                    Some(owner) if owner == package => return None,
                    Some(owner) => Owner::Package(owner.to_string()),
                    None => {
                        let path = bin_dir.join(exe);
                        if !(path.exists() || path.is_symlink()) {
                            return None;
                        }
                        Owner::Unmanaged
                    }
                };
                Some(Collision {
                    executable: exe.clone(),
                    owner,
                })
            })
            .collect()
    }

    /// Drop `executable` from whichever package currently claims it, removing
    /// that package entirely if it has nothing left. Used by `--force` when a
    /// new install takes over a name.
    pub fn release(&mut self, executable: &str) -> Option<String> {
        let owner = self.owner_of(executable)?.to_string();
        let now_empty = match self.packages.get_mut(&owner) {
            Some(pkg) => {
                pkg.executables.retain(|e| e != executable);
                pkg.executables.is_empty()
            }
            None => false,
        };
        if now_empty {
            self.packages.remove(&owner);
        }
        Some(owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(version: &str, exes: &[&str]) -> InstalledPackage {
        InstalledPackage {
            version: version.to_string(),
            executables: exes.iter().map(|s| s.to_string()).collect(),
            ghc: Some("9.8.2".to_string()),
            source: InstallSource::Hackage,
            installed_at: DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    fn names(exes: &[&str]) -> Vec<String> {
        exes.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn missing_manifest_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = InstalledManifest::load(&dir.path().join("installed.toml")).unwrap();
        assert!(manifest.packages.is_empty());
    }

    #[test]
    fn manifest_round_trips_every_source_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("installed.toml");

        let mut manifest = InstalledManifest::default();
        manifest
            .packages
            .insert("hlint".to_string(), pkg("3.8", &["hlint"]));
        let mut git = pkg("0.1.0", &["tool"]);
        git.source = InstallSource::Git {
            url: "https://example.com/tool.git".to_string(),
            rev: Some("0123456789abcdef".to_string()),
        };
        git.ghc = None;
        manifest.packages.insert("tool".to_string(), git);
        let mut local = pkg("1.0", &["mine", "mine-helper"]);
        local.source = InstallSource::Path {
            path: PathBuf::from("/src/mine"),
        };
        manifest.packages.insert("mine".to_string(), local);

        manifest.save(&path).unwrap();
        let loaded = InstalledManifest::load(&path).unwrap();
        assert_eq!(loaded, manifest);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[packages.hlint]"), "{text}");
        assert!(text.contains("kind = \"hackage\""), "{text}");
    }

    #[test]
    fn corrupt_manifest_is_a_config_error_with_a_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.toml");
        std::fs::write(&path, "packages = 3").unwrap();
        let err = InstalledManifest::load(&path).unwrap_err();
        assert_eq!(err.code().exit_code(), 3);
        assert!(!err.fixes().is_empty());
    }

    #[test]
    fn owner_of_finds_the_installing_package() {
        let mut manifest = InstalledManifest::default();
        manifest
            .packages
            .insert("fourmolu".to_string(), pkg("0.18.0", &["fourmolu"]));
        assert_eq!(manifest.owner_of("fourmolu"), Some("fourmolu"));
        assert_eq!(manifest.owner_of("ormolu"), None);
    }

    #[test]
    fn collision_with_another_package() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = InstalledManifest::default();
        manifest
            .packages
            .insert("ormolu".to_string(), pkg("0.7", &["ormolu"]));

        let got = manifest.collisions("ormolu-fork", &names(&["ormolu"]), dir.path());
        assert_eq!(
            got,
            vec![Collision {
                executable: "ormolu".to_string(),
                owner: Owner::Package("ormolu".to_string()),
            }]
        );
    }

    #[test]
    fn collision_with_unmanaged_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ghc"), "").unwrap();
        let manifest = InstalledManifest::default();

        let got = manifest.collisions("ghc", &names(&["ghc", "ghc-other"]), dir.path());
        assert_eq!(
            got,
            vec![Collision {
                executable: "ghc".to_string(),
                owner: Owner::Unmanaged,
            }]
        );
    }

    #[test]
    fn reinstalling_own_executables_is_not_a_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hlint"), "").unwrap();
        let mut manifest = InstalledManifest::default();
        manifest
            .packages
            .insert("hlint".to_string(), pkg("3.6", &["hlint"]));

        assert!(
            manifest
                .collisions("hlint", &names(&["hlint"]), dir.path())
                .is_empty()
        );
    }

    #[test]
    fn release_moves_ownership_and_drops_emptied_packages() {
        let mut manifest = InstalledManifest::default();
        manifest
            .packages
            .insert("a".to_string(), pkg("1", &["shared", "a-only"]));
        manifest
            .packages
            .insert("b".to_string(), pkg("1", &["b-only"]));

        assert_eq!(manifest.release("shared"), Some("a".to_string()));
        assert_eq!(manifest.packages["a"].executables, names(&["a-only"]));

        assert_eq!(manifest.release("b-only"), Some("b".to_string()));
        assert!(!manifest.packages.contains_key("b"));

        assert_eq!(manifest.release("nobody"), None);
    }

    #[test]
    fn describe_sources() {
        assert_eq!(InstallSource::Hackage.describe(), "hackage");
        let git = InstallSource::Git {
            url: "https://x/y.git".to_string(),
            rev: Some("0123456789abcdef0123".to_string()),
        };
        assert_eq!(git.describe(), "git https://x/y.git @ 0123456789ab");
    }
}
