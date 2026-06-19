//! Cabal file parser for extracting build information.
//!
//! This module parses .cabal files to extract both dependencies and full build
//! configuration needed for native compilation.

use crate::package::Dependency;
use crate::version::{VersionConstraint, parse_constraint};
use serde::{Deserialize, Serialize};

/// Parsed information from a .cabal file (minimal, for dependency resolution).
#[derive(Debug, Clone, Default)]
pub struct CabalFile {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Build dependencies from the library section
    pub library_deps: Vec<Dependency>,
    /// Build dependencies from executable sections
    pub executable_deps: Vec<Dependency>,
}

impl CabalFile {
    /// Get all unique dependencies.
    pub fn all_dependencies(&self) -> Vec<Dependency> {
        let mut deps = self.library_deps.clone();
        for dep in &self.executable_deps {
            if !deps.iter().any(|d| d.name == dep.name) {
                deps.push(dep.clone());
            }
        }
        deps
    }
}

/// Full build information extracted from a .cabal file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageBuildInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Build type (Simple, Custom, Configure, Make)
    pub build_type: BuildType,
    /// Library configuration (if package has a library)
    pub library: Option<LibraryConfig>,
    /// Executable configurations
    pub executables: Vec<ExecutableConfig>,
    /// Cabal version specification
    pub cabal_version: Option<String>,
    /// Custom Setup.hs configuration (for build-type: Custom)
    pub custom_setup: Option<CustomSetupConfig>,
}

/// Configuration for custom Setup.hs dependencies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomSetupConfig {
    /// Dependencies required to build Setup.hs
    pub setup_depends: Vec<Dependency>,
}

impl PackageBuildInfo {
    /// Check if this package can be built natively (without custom Setup.hs).
    pub fn is_simple_build(&self) -> bool {
        matches!(self.build_type, BuildType::Simple)
    }

    /// Check if this package requires preprocessors we don't support.
    ///
    /// We now support alex, happy, and hsc2hs natively. Only c2hs and cpphs
    /// are still unsupported.
    pub fn needs_unsupported_preprocessors(&self) -> bool {
        let check_tools = |tools: &[String]| {
            tools.iter().any(|t| {
                let t = t.to_lowercase();
                // c2hs and cpphs are still unsupported
                t.contains("c2hs") || t.contains("cpphs")
            })
        };

        if let Some(lib) = &self.library
            && check_tools(&lib.build_tools)
        {
            return true;
        }

        for exe in &self.executables {
            if check_tools(&exe.build_tools) {
                return true;
            }
        }

        false
    }

    /// Get the list of preprocessors needed by this package.
    pub fn needed_preprocessors(&self) -> Vec<&'static str> {
        let mut preprocessors = Vec::new();

        let collect_from_tools = |tools: &[String], preprocessors: &mut Vec<&'static str>| {
            for tool in tools {
                let t = tool.to_lowercase();
                if t.contains("alex") && !preprocessors.contains(&"alex") {
                    preprocessors.push("alex");
                }
                if t.contains("happy") && !preprocessors.contains(&"happy") {
                    preprocessors.push("happy");
                }
                if t.contains("hsc2hs") && !preprocessors.contains(&"hsc2hs") {
                    preprocessors.push("hsc2hs");
                }
            }
        };

        if let Some(lib) = &self.library {
            collect_from_tools(&lib.build_tools, &mut preprocessors);
        }

        for exe in &self.executables {
            collect_from_tools(&exe.build_tools, &mut preprocessors);
        }

        preprocessors
    }

    /// Get all dependencies for building the library.
    pub fn library_dependencies(&self) -> Vec<Dependency> {
        self.library
            .as_ref()
            .map(|lib| lib.build_depends.clone())
            .unwrap_or_default()
    }
}

/// Build type for a package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildType {
    /// Simple build (no custom Setup.hs)
    #[default]
    Simple,
    /// Custom Setup.hs required
    Custom,
    /// Configure script required
    Configure,
    /// Makefile-based build
    Make,
}

impl BuildType {
    fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "simple" => BuildType::Simple,
            "custom" => BuildType::Custom,
            "configure" => BuildType::Configure,
            "make" => BuildType::Make,
            _ => BuildType::Simple,
        }
    }
}

/// Library configuration from a .cabal file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryConfig {
    /// Source directories (hs-source-dirs)
    pub hs_source_dirs: Vec<String>,
    /// Exposed modules
    pub exposed_modules: Vec<String>,
    /// Other (internal) modules
    pub other_modules: Vec<String>,
    /// Build dependencies
    pub build_depends: Vec<Dependency>,
    /// Default language extensions
    pub default_extensions: Vec<String>,
    /// Other extensions
    pub other_extensions: Vec<String>,
    /// GHC options
    pub ghc_options: Vec<String>,
    /// CPP options (preprocessor flags)
    pub cpp_options: Vec<String>,
    /// CC options (C compiler flags)
    pub cc_options: Vec<String>,
    /// C source files
    pub c_sources: Vec<String>,
    /// Include directories for C headers
    pub include_dirs: Vec<String>,
    /// Header files to include
    pub includes: Vec<String>,
    /// Extra C libraries to link
    pub extra_libraries: Vec<String>,
    /// Extra library directories
    pub extra_lib_dirs: Vec<String>,
    /// pkg-config dependencies
    pub pkgconfig_depends: Vec<String>,
    /// Build tools required
    pub build_tools: Vec<String>,
    /// Default language (Haskell98, Haskell2010, GHC2021)
    pub default_language: Option<String>,
}

/// Executable configuration from a .cabal file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutableConfig {
    /// Executable name
    pub name: String,
    /// Main module file
    pub main_is: Option<String>,
    /// Source directories
    pub hs_source_dirs: Vec<String>,
    /// Other modules
    pub other_modules: Vec<String>,
    /// Build dependencies
    pub build_depends: Vec<Dependency>,
    /// Default language extensions
    pub default_extensions: Vec<String>,
    /// GHC options
    pub ghc_options: Vec<String>,
    /// CPP options (preprocessor flags)
    pub cpp_options: Vec<String>,
    /// CC options (C compiler flags)
    pub cc_options: Vec<String>,
    /// C source files
    pub c_sources: Vec<String>,
    /// Include directories for C headers
    pub include_dirs: Vec<String>,
    /// Extra libraries
    pub extra_libraries: Vec<String>,
    /// Build tools required
    pub build_tools: Vec<String>,
    /// Default language
    pub default_language: Option<String>,
}

/// Parse a .cabal file and extract full build information.
pub fn parse_cabal_full(content: &str) -> PackageBuildInfo {
    let mut info = PackageBuildInfo::default();
    let mut current_section = Section::TopLevel;
    let mut current_library = LibraryConfig::default();
    let mut current_executable: Option<ExecutableConfig> = None;
    let mut current_custom_setup = CustomSetupConfig::default();
    let mut field_buffer = FieldBuffer::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with("--") || trimmed.is_empty() {
            continue;
        }

        // Check for section headers
        if let Some(section) = parse_section_header(trimmed) {
            // Flush any buffered field
            flush_field(
                &mut field_buffer,
                &current_section,
                &mut info,
                &mut current_library,
                &mut current_executable,
                &mut current_custom_setup,
            );

            // Save current section data
            match current_section {
                Section::Library => {
                    info.library = Some(std::mem::take(&mut current_library));
                }
                Section::Executable(_) => {
                    if let Some(exe) = current_executable.take() {
                        info.executables.push(exe);
                    }
                }
                Section::CustomSetup if !current_custom_setup.setup_depends.is_empty() => {
                    info.custom_setup = Some(std::mem::take(&mut current_custom_setup));
                }
                _ => {}
            }

            // Start new section
            current_section = section.clone();
            if let Section::Executable(name) = &section {
                current_executable = Some(ExecutableConfig {
                    name: name.clone(),
                    ..Default::default()
                });
            }
            continue;
        }

        // Check if this is a new field or continuation
        if let Some((key, value)) = parse_field(line) {
            // Flush previous field
            flush_field(
                &mut field_buffer,
                &current_section,
                &mut info,
                &mut current_library,
                &mut current_executable,
                &mut current_custom_setup,
            );
            // Start new field
            field_buffer.start(key, value);
        } else if line.starts_with(' ') || line.starts_with('\t') {
            // Continuation line
            field_buffer.append(trimmed);
        }
    }

    // Flush final field
    flush_field(
        &mut field_buffer,
        &current_section,
        &mut info,
        &mut current_library,
        &mut current_executable,
        &mut current_custom_setup,
    );

    // Save final section
    match current_section {
        Section::Library => {
            info.library = Some(current_library);
        }
        Section::Executable(_) => {
            if let Some(exe) = current_executable {
                info.executables.push(exe);
            }
        }
        Section::CustomSetup if !current_custom_setup.setup_depends.is_empty() => {
            info.custom_setup = Some(current_custom_setup);
        }
        _ => {}
    }

    info
}

/// Buffer for accumulating multi-line field values.
struct FieldBuffer {
    key: String,
    value: String,
    active: bool,
}

impl FieldBuffer {
    fn new() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            active: false,
        }
    }

    fn start(&mut self, key: &str, value: &str) {
        self.key = key.to_lowercase();
        self.value = value.to_string();
        self.active = true;
    }

    fn append(&mut self, line: &str) {
        if self.active {
            if !self.value.is_empty() {
                self.value.push(' ');
            }
            self.value.push_str(line);
        }
    }

    fn take(&mut self) -> Option<(String, String)> {
        if self.active {
            self.active = false;
            Some((
                std::mem::take(&mut self.key),
                std::mem::take(&mut self.value),
            ))
        } else {
            None
        }
    }
}

/// Flush accumulated field value to appropriate structure.
fn flush_field(
    buffer: &mut FieldBuffer,
    section: &Section,
    info: &mut PackageBuildInfo,
    library: &mut LibraryConfig,
    executable: &mut Option<ExecutableConfig>,
    custom_setup: &mut CustomSetupConfig,
) {
    let Some((key, value)) = buffer.take() else {
        return;
    };

    match section {
        Section::TopLevel => {
            apply_top_level_field(info, &key, &value);
        }
        Section::Library => {
            apply_library_field(library, &key, &value);
        }
        Section::Executable(_) => {
            if let Some(exe) = executable {
                apply_executable_field(exe, &key, &value);
            }
        }
        Section::CustomSetup => {
            apply_custom_setup_field(custom_setup, &key, &value);
        }
        Section::Other => {}
    }
}

/// Apply a custom-setup section field to CustomSetupConfig.
fn apply_custom_setup_field(setup: &mut CustomSetupConfig, key: &str, value: &str) {
    if key == "setup-depends" {
        setup.setup_depends = parse_build_depends(value);
    }
}

/// Apply a top-level field to PackageBuildInfo.
fn apply_top_level_field(info: &mut PackageBuildInfo, key: &str, value: &str) {
    match key {
        "name" => info.name = value.to_string(),
        "version" => info.version = value.to_string(),
        "build-type" => info.build_type = BuildType::from_str(value),
        "cabal-version" => info.cabal_version = Some(value.to_string()),
        _ => {}
    }
}

/// Apply a library section field to LibraryConfig.
fn apply_library_field(lib: &mut LibraryConfig, key: &str, value: &str) {
    match key {
        "hs-source-dirs" => lib.hs_source_dirs = parse_list(value),
        "exposed-modules" => lib.exposed_modules = parse_module_list(value),
        "other-modules" => lib.other_modules = parse_module_list(value),
        "build-depends" => lib.build_depends = parse_build_depends(value),
        "default-extensions" => lib.default_extensions = parse_list(value),
        "other-extensions" => lib.other_extensions = parse_list(value),
        "ghc-options" => lib.ghc_options = parse_ghc_options(value),
        "cpp-options" => lib.cpp_options = parse_ghc_options(value),
        "cc-options" => lib.cc_options = parse_ghc_options(value),
        "c-sources" => lib.c_sources = parse_list(value),
        "include-dirs" => lib.include_dirs = parse_list(value),
        "includes" => lib.includes = parse_list(value),
        "extra-libraries" => lib.extra_libraries = parse_list(value),
        "extra-lib-dirs" => lib.extra_lib_dirs = parse_list(value),
        "pkgconfig-depends" => lib.pkgconfig_depends = parse_list(value),
        "build-tools" | "build-tool-depends" => lib.build_tools.extend(parse_list(value)),
        "default-language" => lib.default_language = Some(value.to_string()),
        _ => {}
    }
}

/// Apply an executable section field to ExecutableConfig.
fn apply_executable_field(exe: &mut ExecutableConfig, key: &str, value: &str) {
    match key {
        "main-is" => exe.main_is = Some(value.to_string()),
        "hs-source-dirs" => exe.hs_source_dirs = parse_list(value),
        "other-modules" => exe.other_modules = parse_module_list(value),
        "build-depends" => exe.build_depends = parse_build_depends(value),
        "default-extensions" => exe.default_extensions = parse_list(value),
        "ghc-options" => exe.ghc_options = parse_ghc_options(value),
        "cpp-options" => exe.cpp_options = parse_ghc_options(value),
        "cc-options" => exe.cc_options = parse_ghc_options(value),
        "c-sources" => exe.c_sources = parse_list(value),
        "include-dirs" => exe.include_dirs = parse_list(value),
        "extra-libraries" => exe.extra_libraries = parse_list(value),
        "build-tools" | "build-tool-depends" => exe.build_tools.extend(parse_list(value)),
        "default-language" => exe.default_language = Some(value.to_string()),
        _ => {}
    }
}

/// Parse a comma or space separated list.
fn parse_list(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse a module list (comma or newline separated).
fn parse_module_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .flat_map(|s| s.split_whitespace())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse GHC options (preserving structure like -Wall -Werror).
fn parse_ghc_options(value: &str) -> Vec<String> {
    // Split on spaces but handle quoted strings
    let mut options = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in value.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    options.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        options.push(trimmed);
    }

    options
}

/// Parse a .cabal file and extract dependencies (original minimal parser).
pub fn parse_cabal(content: &str) -> CabalFile {
    let mut result = CabalFile::default();
    let lines = content.lines();
    let mut current_section = Section::TopLevel;
    let mut in_build_depends = false;
    let mut build_depends_buffer = String::new();
    // Indentation (in leading-whitespace columns) of the active `build-depends`
    // field. Cabal's layout rule: a line continues the field only when indented
    // strictly deeper than the field; a line at the same-or-lesser indent starts
    // a new field. Using `starts_with(' ')` instead glues sibling fields (e.g.
    // `default-language:`) onto the dependency list.
    let mut build_depends_indent = 0;

    for line in lines {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with("--") || trimmed.is_empty() {
            continue;
        }

        // Check for section headers
        if let Some(section) = parse_section_header(trimmed) {
            // Process any buffered build-depends before switching sections
            if in_build_depends && !build_depends_buffer.is_empty() {
                let deps = parse_build_depends(&build_depends_buffer);
                match current_section {
                    Section::Library => result.library_deps.extend(deps),
                    Section::Executable(_) => result.executable_deps.extend(deps),
                    _ => {}
                }
                build_depends_buffer.clear();
            }
            in_build_depends = false;
            current_section = section;
            continue;
        }

        // Parse top-level fields
        if matches!(current_section, Section::TopLevel) {
            if let Some((key, value)) = parse_field(line) {
                match key.to_lowercase().as_str() {
                    "name" => result.name = value.to_string(),
                    "version" => result.version = value.to_string(),
                    _ => {}
                }
            }
            continue;
        }

        // Parse build-depends in library/executable sections
        if matches!(current_section, Section::Library | Section::Executable(_)) {
            // A line indented deeper than the active build-depends field
            // continues it; anything else terminates it (and may begin a new
            // field). Detect new fields by indentation first so a sibling field
            // never bleeds into the dependency buffer.
            if in_build_depends && indent_width(line) > build_depends_indent {
                if !build_depends_buffer.is_empty() {
                    build_depends_buffer.push(' ');
                }
                build_depends_buffer.push_str(trimmed);
                continue;
            }

            // Not a continuation: flush any pending build-depends before this
            // line is interpreted as a new field.
            if in_build_depends {
                let deps = parse_build_depends(&build_depends_buffer);
                match current_section {
                    Section::Library => result.library_deps.extend(deps),
                    Section::Executable(_) => result.executable_deps.extend(deps),
                    _ => {}
                }
                build_depends_buffer.clear();
                in_build_depends = false;
            }

            // A new build-depends field starts here.
            if let Some((key, value)) = parse_field(line)
                && key.to_lowercase() == "build-depends"
            {
                in_build_depends = true;
                build_depends_indent = indent_width(line);
                build_depends_buffer = value.to_string();
            }
        }
    }

    // Process any remaining build-depends
    if in_build_depends && !build_depends_buffer.is_empty() {
        let deps = parse_build_depends(&build_depends_buffer);
        match current_section {
            Section::Library => result.library_deps.extend(deps),
            Section::Executable(_) => result.executable_deps.extend(deps),
            _ => {}
        }
    }

    result
}

#[derive(Debug, Clone)]
enum Section {
    TopLevel,
    Library,
    #[allow(dead_code)] // Name stored for future use
    Executable(String),
    CustomSetup,
    Other,
}

fn parse_section_header(line: &str) -> Option<Section> {
    let lower = line.to_lowercase();

    if lower == "library" {
        return Some(Section::Library);
    }

    if lower == "custom-setup" {
        return Some(Section::CustomSetup);
    }

    if lower.starts_with("executable ") {
        let name = line[11..].trim().to_string();
        return Some(Section::Executable(name));
    }

    if lower.starts_with("test-suite ")
        || lower.starts_with("benchmark ")
        || lower.starts_with("common ")
        || lower.starts_with("source-repository ")
        || lower.starts_with("flag ")
    {
        return Some(Section::Other);
    }

    None
}

/// Leading-whitespace width of a line, used for Cabal's layout rule.
fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn parse_field(line: &str) -> Option<(&str, &str)> {
    let colon_pos = line.find(':')?;
    let key = line[..colon_pos].trim();
    let value = line[colon_pos + 1..].trim();
    Some((key, value))
}

/// Parse a build-depends field value into dependencies.
fn parse_build_depends(value: &str) -> Vec<Dependency> {
    let mut deps = Vec::new();

    // Split on commas, but not commas inside brackets/braces/parens — Cabal's
    // set-version notation puts commas inside braces, e.g.
    // `base ^>= {4.14, 4.17}` is ONE dependency, not two.
    let mut depth: i32 = 0;
    let mut start = 0;
    let push = |seg: &str, out: &mut Vec<Dependency>| {
        let seg = seg.trim();
        if !seg.is_empty()
            && let Some(dep) = parse_single_dependency(seg)
        {
            out.push(dep);
        }
    };
    for (i, c) in value.char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                push(&value[start..i], &mut deps);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&value[start..], &mut deps);

    deps
}

/// Parse a single dependency like "base >= 4.7 && < 5" or "text"
fn parse_single_dependency(s: &str) -> Option<Dependency> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // The package name runs until the first whitespace, version operator, or
    // parenthesis. Names may contain letters, digits, '-', '.', and ':' (for
    // the `package:sublibrary` form) but none of those terminators — so e.g.
    // `base (>= 4.9 && < 5)` yields the name `base`, not `base (`.
    let name_end = s
        .find(|c: char| c.is_whitespace() || matches!(c, '>' | '<' | '=' | '^' | '(' | ')'))
        .unwrap_or(s.len());

    let name = s[..name_end].trim();
    if name.is_empty() {
        return None;
    }

    // Handle library subcomponent syntax: package:library
    let (package_name, library) = if let Some(colon_pos) = name.find(':') {
        let pkg = &name[..colon_pos];
        let lib = &name[colon_pos + 1..];
        (pkg.trim(), Some(lib.trim().to_string()))
    } else {
        (name, None)
    };

    // Cabal permits parenthesised constraints, e.g. `base (>= 4.9 && < 5)`.
    // Parentheses are only grouping; with our `||`-then-`&&` precedence the
    // common forms parse correctly once the parens are removed.
    let constraint_str: String = s[name_end..].chars().filter(|c| !matches!(c, '(' | ')')).collect();
    let constraint_str = constraint_str.trim();
    let constraint = if constraint_str.is_empty() {
        VersionConstraint::Any
    } else {
        match parse_constraint(constraint_str) {
            Ok(c) => c,
            Err(e) => {
                // Logged at debug, not warn: parsing the full Hackage index
                // means encountering exotic/malformed metadata in packages the
                // user may not even depend on (brace-layout dependency lists,
                // dash-separated or "infinity" versions, …). Treating these as
                // unconstrained is the graceful fallback and not actionable for
                // the user, so it must not spam `hx lock`/`hx outdated`.
                tracing::debug!(
                    "Could not parse version constraint '{}' for package '{}' ({}); treating as unconstrained",
                    constraint_str,
                    package_name,
                    e
                );
                VersionConstraint::Any
            }
        }
    };

    Some(Dependency {
        name: package_name.to_string(),
        constraint,
        library,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_cabal() {
        let content = r#"
name:           text
version:        2.1.1
build-type:     Simple

library
  build-depends:
    base >= 4.9 && < 5,
    bytestring >= 0.10.4,
    deepseq
"#;

        let cabal = parse_cabal(content);
        assert_eq!(cabal.name, "text");
        assert_eq!(cabal.version, "2.1.1");
        assert_eq!(cabal.library_deps.len(), 3);

        let base_dep = cabal
            .library_deps
            .iter()
            .find(|d| d.name == "base")
            .unwrap();
        assert!(matches!(base_dep.constraint, VersionConstraint::And(_, _)));
    }

    #[test]
    fn test_build_depends_does_not_bleed_into_sibling_fields() {
        // Regression: sibling fields at the same indentation as `build-depends`
        // (here `default-language` / `hs-source-dirs`) were being appended to
        // the dependency list, producing constraints like
        // `>= 3 && < 4Hs-Source-Dirs: src`. Cabal's layout rule terminates the
        // field at the first line that is not indented deeper.
        let content = r#"
name:    spacecookie
version: 1.0.0

library
  build-depends:    base >= 3 && < 4
  default-language: Haskell2010
  hs-source-dirs:   src
  exposed-modules:  Foo
"#;

        let cabal = parse_cabal(content);
        assert_eq!(cabal.library_deps.len(), 1);
        let base = &cabal.library_deps[0];
        assert_eq!(base.name, "base");
        // Parses cleanly as a range, not a bled-together unconstrained mess.
        assert!(matches!(base.constraint, VersionConstraint::And(_, _)));
    }

    #[test]
    fn test_build_depends_parenthesised_and_set_constraints() {
        // Parenthesised constraint and Cabal set-version notation (with its
        // comma *inside* the braces) must each parse as a single dependency.
        let content = r#"
name:    pkg
version: 1.0.0

library
  build-depends: base (>= 4.9 && < 5)
               , containers ^>= {0.6, 0.7}
  default-language: Haskell2010
"#;

        let cabal = parse_cabal(content);
        // Exactly two deps — the comma inside `{0.6, 0.7}` must NOT split.
        assert_eq!(cabal.library_deps.len(), 2);

        let base = cabal.library_deps.iter().find(|d| d.name == "base").unwrap();
        assert_eq!(base.name, "base"); // not "base ("
        assert!(matches!(base.constraint, VersionConstraint::And(_, _)));

        let containers = cabal
            .library_deps
            .iter()
            .find(|d| d.name == "containers")
            .unwrap();
        assert!(matches!(containers.constraint, VersionConstraint::Or(_, _)));
    }

    #[test]
    fn test_build_depends_wildcard_constraint() {
        // Regression: Cabal wildcard constraints (`== 0.5.*`) were rejected and
        // silently downgraded to unconstrained.
        let content = r#"
name:    pkg
version: 1.0.0

library
  build-depends: containers == 0.5.* || == 0.6.*,
                 base
  default-language: Haskell2010
"#;

        let cabal = parse_cabal(content);
        let containers = cabal
            .library_deps
            .iter()
            .find(|d| d.name == "containers")
            .unwrap();
        // Two wildcard alternatives -> an Or of And ranges, not Any.
        assert!(matches!(containers.constraint, VersionConstraint::Or(_, _)));
    }

    #[test]
    fn test_parse_full_cabal() {
        let content = r#"
cabal-version:  2.4
name:           mylib
version:        1.0.0
build-type:     Simple

library
  hs-source-dirs:   src
  exposed-modules:  MyLib, MyLib.Internal
  other-modules:    MyLib.Utils
  build-depends:    base >= 4.14 && < 5,
                    text >= 2.0
  default-extensions: OverloadedStrings, DeriveFunctor
  ghc-options:      -Wall -Werror
  default-language: GHC2021

executable myapp
  main-is:          Main.hs
  hs-source-dirs:   app
  build-depends:    base, mylib
  ghc-options:      -threaded -rtsopts
"#;

        let info = parse_cabal_full(content);
        assert_eq!(info.name, "mylib");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.build_type, BuildType::Simple);
        assert!(info.is_simple_build());

        let lib = info.library.as_ref().unwrap();
        assert_eq!(lib.hs_source_dirs, vec!["src"]);
        assert_eq!(lib.exposed_modules, vec!["MyLib", "MyLib.Internal"]);
        assert_eq!(lib.other_modules, vec!["MyLib.Utils"]);
        assert_eq!(lib.build_depends.len(), 2);
        assert_eq!(
            lib.default_extensions,
            vec!["OverloadedStrings", "DeriveFunctor"]
        );
        assert_eq!(lib.ghc_options, vec!["-Wall", "-Werror"]);
        assert_eq!(lib.default_language, Some("GHC2021".to_string()));

        assert_eq!(info.executables.len(), 1);
        let exe = &info.executables[0];
        assert_eq!(exe.name, "myapp");
        assert_eq!(exe.main_is, Some("Main.hs".to_string()));
        assert_eq!(exe.hs_source_dirs, vec!["app"]);
        assert_eq!(exe.ghc_options, vec!["-threaded", "-rtsopts"]);
    }

    #[test]
    fn test_parse_custom_build_type() {
        let content = r#"
name:       custom-pkg
version:    1.0
build-type: Custom
"#;

        let info = parse_cabal_full(content);
        assert_eq!(info.build_type, BuildType::Custom);
        assert!(!info.is_simple_build());
    }

    #[test]
    fn test_detect_preprocessors() {
        let content = r#"
name:       alex-pkg
version:    1.0
build-type: Simple

library
  build-tools: alex, happy
"#;

        let info = parse_cabal_full(content);
        // alex and happy are now supported, so this should return false
        assert!(!info.needs_unsupported_preprocessors());
        // But we should detect them as needed preprocessors
        let needed = info.needed_preprocessors();
        assert!(needed.contains(&"alex"));
        assert!(needed.contains(&"happy"));
    }

    #[test]
    fn test_unsupported_preprocessors() {
        let content = r#"
name:       c2hs-pkg
version:    1.0
build-type: Simple

library
  build-tools: c2hs
"#;

        let info = parse_cabal_full(content);
        // c2hs is still unsupported
        assert!(info.needs_unsupported_preprocessors());
    }

    #[test]
    fn test_parse_c_sources() {
        let content = r#"
name:       ffi-pkg
version:    1.0
build-type: Simple

library
  c-sources:        cbits/foo.c, cbits/bar.c
  include-dirs:     include
  extra-libraries:  pthread, m
"#;

        let info = parse_cabal_full(content);
        let lib = info.library.as_ref().unwrap();
        assert_eq!(lib.c_sources, vec!["cbits/foo.c", "cbits/bar.c"]);
        assert_eq!(lib.include_dirs, vec!["include"]);
        assert_eq!(lib.extra_libraries, vec!["pthread", "m"]);
    }

    #[test]
    fn test_parse_custom_setup() {
        let content = r#"
name:       custom-pkg
version:    1.0
build-type: Custom

custom-setup
  setup-depends: base >= 4.10 && < 5,
                 Cabal >= 2.0,
                 directory

library
  exposed-modules: Custom
"#;

        let info = parse_cabal_full(content);
        assert_eq!(info.build_type, BuildType::Custom);
        assert!(!info.is_simple_build());

        let setup = info
            .custom_setup
            .as_ref()
            .expect("custom_setup should be present");
        assert_eq!(setup.setup_depends.len(), 3);
        assert!(setup.setup_depends.iter().any(|d| d.name == "base"));
        assert!(setup.setup_depends.iter().any(|d| d.name == "Cabal"));
        assert!(setup.setup_depends.iter().any(|d| d.name == "directory"));
    }

    #[test]
    fn test_parse_dependency_with_constraint() {
        let dep = parse_single_dependency("base >= 4.7 && < 5").unwrap();
        assert_eq!(dep.name, "base");
        assert!(matches!(dep.constraint, VersionConstraint::And(_, _)));
    }

    #[test]
    fn test_parse_dependency_no_constraint() {
        let dep = parse_single_dependency("text").unwrap();
        assert_eq!(dep.name, "text");
        assert!(matches!(dep.constraint, VersionConstraint::Any));
    }

    #[test]
    fn test_parse_dependency_with_library() {
        let dep = parse_single_dependency("containers:containers >= 0.6").unwrap();
        assert_eq!(dep.name, "containers");
        assert_eq!(dep.library, Some("containers".to_string()));
    }

    #[test]
    fn test_parse_caret_constraint() {
        let dep = parse_single_dependency("aeson ^>= 2.2").unwrap();
        assert_eq!(dep.name, "aeson");
        assert!(matches!(dep.constraint, VersionConstraint::Caret(_)));
    }

    #[test]
    fn test_parse_build_depends_multiline() {
        let value = "base >= 4.9, text >= 2.0, bytestring";
        let deps = parse_build_depends(value);
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn test_parse_ghc_options() {
        let options = parse_ghc_options("-Wall -Werror -O2");
        assert_eq!(options, vec!["-Wall", "-Werror", "-O2"]);
    }

    #[test]
    fn test_parse_module_list() {
        let modules =
            parse_module_list("Data.Text, Data.Text.Lazy,\n                    Data.Text.Internal");
        assert_eq!(
            modules,
            vec!["Data.Text", "Data.Text.Lazy", "Data.Text.Internal"]
        );
    }
}
