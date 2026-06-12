//! Cabal file editing utilities.
//!
//! Provides simple text-based editing of .cabal files for adding and removing
//! dependencies. Uses a line-based approach to preserve formatting.

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CabalEditError {
    #[error("failed to read cabal file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("no build-depends section found in cabal file")]
    NoBuildDepends,

    #[error("dependency '{0}' not found")]
    DependencyNotFound(String),

    #[error("dependency '{0}' already exists")]
    DependencyExists(String),
}

/// Add a dependency to a .cabal file.
///
/// Finds the first `build-depends:` section and adds the package there.
/// Supports both leading-comma style (`, base`) and trailing-comma style (`base,`).
pub fn add_dependency(
    cabal_path: impl AsRef<Path>,
    package: &str,
    version_constraint: Option<&str>,
) -> Result<(), CabalEditError> {
    let content = std::fs::read_to_string(cabal_path.as_ref())?;
    let lines: Vec<&str> = content.lines().collect();

    // Find the build-depends line
    let mut build_depends_idx = None;
    let mut indent = "        ";
    let mut uses_leading_comma = false;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("build-depends:") {
            build_depends_idx = Some(i);
            // Detect indentation and comma style from next line if possible
            if i + 1 < lines.len() {
                let next_line = lines[i + 1];
                let spaces = next_line.len() - next_line.trim_start().len();
                if spaces > 0 {
                    indent = &lines[i + 1][..spaces];
                    uses_leading_comma = next_line.trim_start().starts_with(',');
                }
            }
            break;
        }
    }

    let build_depends_idx = build_depends_idx.ok_or(CabalEditError::NoBuildDepends)?;

    // Check if dependency already exists
    let package_lower = package.to_lowercase();
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with(&package_lower)
            || trimmed.starts_with(&format!(",{}", package_lower))
            || trimmed.starts_with(&format!(", {}", package_lower))
            || trimmed.contains(&format!(" {}", package_lower))
        {
            // Simple check - might have false positives but good enough for common cases
            let dep_part = trimmed.trim_start_matches(',').trim();
            let dep_part_lower = dep_part.to_lowercase();
            if dep_part_lower.starts_with(&package_lower) {
                // Index the lowercased string: byte offsets of the original may
                // not align (or even be char boundaries) for non-ASCII names
                let after_name = &dep_part_lower[package_lower.len()..];
                // Make sure it's actually this package and not a prefix (e.g., "text" vs "text-show")
                if after_name.is_empty()
                    || after_name.starts_with(' ')
                    || after_name.starts_with(',')
                {
                    return Err(CabalEditError::DependencyExists(package.to_string()));
                }
            }
        }
    }

    // Find where to insert (after the last dependency in this section)
    let mut insert_idx = build_depends_idx + 1;
    let mut last_dep_idx = build_depends_idx;

    for (i, line) in lines.iter().enumerate().skip(build_depends_idx + 1) {
        let trimmed = line.trim();
        // Check if we're still in the build-depends list
        if trimmed.is_empty() {
            continue;
        } else if trimmed.starts_with(',') {
            // Leading comma style continuation
            insert_idx = i + 1;
            last_dep_idx = i;
        } else if !line.starts_with(' ') && !line.starts_with('\t') {
            // Hit a new section (no indentation)
            break;
        } else if trimmed.contains(':') && !trimmed.starts_with(',') {
            // Hit a new field
            break;
        } else {
            // Still in build-depends (trailing comma style or first dep after build-depends:)
            insert_idx = i + 1;
            last_dep_idx = i;
        }
    }

    // Reconstruct the file
    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    // Build the new dependency
    let dep_value = if let Some(constraint) = version_constraint {
        format!("{} {}", package, constraint)
    } else {
        package.to_string()
    };

    if uses_leading_comma {
        // Leading comma style: add ", package" on new line
        let dep_line = format!("{}, {}", indent, dep_value);
        new_lines.insert(insert_idx, dep_line);
    } else {
        // Trailing comma style: add comma to previous line, then add package on new line
        // First, add a comma to the last dependency line if it doesn't have one
        if last_dep_idx > build_depends_idx {
            let last_line = &new_lines[last_dep_idx];
            let trimmed = last_line.trim_end();
            if !trimmed.ends_with(',') {
                new_lines[last_dep_idx] = format!("{},", trimmed);
            }
        } else {
            // Dependencies are inline with build-depends:
            let bd_line = &new_lines[build_depends_idx];
            let trimmed = bd_line.trim_end();
            if !trimmed.ends_with(',') {
                new_lines[build_depends_idx] = format!("{},", trimmed);
            }
        }
        let dep_line = format!("{}{}", indent, dep_value);
        new_lines.insert(insert_idx, dep_line);
    }

    let new_content = new_lines.join("\n");
    std::fs::write(cabal_path.as_ref(), new_content)?;

    Ok(())
}

/// Remove a dependency from a .cabal file.
pub fn remove_dependency(
    cabal_path: impl AsRef<Path>,
    package: &str,
) -> Result<(), CabalEditError> {
    let content = std::fs::read_to_string(cabal_path.as_ref())?;
    let lines: Vec<&str> = content.lines().collect();
    let package_lower = package.to_lowercase();

    let mut found = false;
    let mut new_lines: Vec<String> = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        let dep_part = trimmed.trim_start_matches(',').trim();

        // Check if this line contains the dependency to remove
        let is_target = dep_part.to_lowercase().starts_with(&package_lower)
            && (dep_part.len() == package_lower.len()
                || dep_part
                    .chars()
                    .nth(package_lower.len())
                    .is_some_and(|c| c == ' ' || c == ','));

        if is_target {
            found = true;
            // Skip this line (don't add to new_lines)
        } else {
            new_lines.push(line.to_string());
        }
    }

    if !found {
        return Err(CabalEditError::DependencyNotFound(package.to_string()));
    }

    let new_content = new_lines.join("\n");
    std::fs::write(cabal_path.as_ref(), new_content)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_add_dependency_leading_comma_style() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        let initial = r#"cabal-version: 3.0
name: test
version: 0.1.0

executable test
  main-is: Main.hs
  build-depends:
    , base >=4.14 && <5
  default-language: GHC2021
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        add_dependency(&cabal_path, "text", Some(">=2.0")).unwrap();

        let result = std::fs::read_to_string(&cabal_path).unwrap();
        assert!(result.contains(", text >=2.0"));
    }

    #[test]
    fn test_add_dependency_trailing_comma_style() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        // This is the style that `hx init` generates
        let initial = r#"cabal-version:      3.0
name:               test
version:            0.1.0.0

executable test
    main-is:          Main.hs
    hs-source-dirs:   src
    default-language: GHC2021
    build-depends:
        base ^>=4.17 || ^>=4.18 || ^>=4.19 || ^>=4.20
    ghc-options:      -Wall
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        add_dependency(&cabal_path, "text", None).unwrap();

        let result = std::fs::read_to_string(&cabal_path).unwrap();
        // Should add comma to base line and add text on new line
        assert!(
            result.contains("^>=4.20,"),
            "should add comma to previous dep"
        );
        assert!(
            result.contains("        text"),
            "should add text with proper indent"
        );
        // Verify the file is still valid (text comes before ghc-options)
        let text_pos = result.find("text").unwrap();
        let ghc_pos = result.find("ghc-options").unwrap();
        assert!(text_pos < ghc_pos, "text should come before ghc-options");
    }

    #[test]
    fn test_add_dependency_with_version_constraint() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        let initial = r#"cabal-version:      3.0
name:               test
version:            0.1.0.0

executable test
    main-is:          Main.hs
    build-depends:
        base ^>=4.17
    ghc-options:      -Wall
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        add_dependency(&cabal_path, "aeson", Some(">=2.0 && <3")).unwrap();

        let result = std::fs::read_to_string(&cabal_path).unwrap();
        assert!(result.contains("aeson >=2.0 && <3"));
    }

    #[test]
    fn test_add_dependency_already_exists() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        let initial = r#"cabal-version: 3.0
name: test
version: 0.1.0

executable test
  main-is: Main.hs
  build-depends:
        base ^>=4.17,
        text >=2.0
  default-language: GHC2021
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        let result = add_dependency(&cabal_path, "text", None);
        assert!(matches!(result, Err(CabalEditError::DependencyExists(_))));
    }

    #[test]
    fn test_add_dependency_similar_name_not_confused() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        let initial = r#"cabal-version: 3.0
name: test
version: 0.1.0

executable test
  main-is: Main.hs
  build-depends:
        base ^>=4.17,
        text >=2.0
  default-language: GHC2021
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        // "text-show" should not be confused with "text"
        let result = add_dependency(&cabal_path, "text-show", None);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&cabal_path).unwrap();
        assert!(content.contains("text-show"));
    }

    #[test]
    fn test_remove_dependency() {
        let dir = tempdir().unwrap();
        let cabal_path = dir.path().join("test.cabal");

        let initial = r#"cabal-version: 3.0
name: test
version: 0.1.0

executable test
  main-is: Main.hs
  build-depends:
    , base >=4.14 && <5
    , text >=2.0
  default-language: GHC2021
"#;
        std::fs::write(&cabal_path, initial).unwrap();

        remove_dependency(&cabal_path, "text").unwrap();

        let result = std::fs::read_to_string(&cabal_path).unwrap();
        assert!(!result.contains("text"));
        assert!(result.contains("base"));
    }
}
