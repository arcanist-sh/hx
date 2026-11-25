//! Module dependency extraction from Haskell source files.
//!
//! This module provides:
//! - Import statement parsing from .hs files
//! - Module dependency graph construction
//! - Topological ordering for compilation

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Error type for module operations.
#[derive(Debug, Error)]
pub enum ModuleError {
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),

    #[error("cycle detected in module dependencies: {0}")]
    CycleDetected(String),

    #[error("module not found: {0}")]
    ModuleNotFound(String),
}

/// Information about a Haskell module.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// Module name (e.g., "Data.List")
    pub name: String,
    /// Path to the source file
    pub path: PathBuf,
    /// Modules this module imports
    pub imports: Vec<String>,
    /// Whether this is a local (project) module
    pub is_local: bool,
}

/// A graph of module dependencies.
#[derive(Debug, Clone, Default)]
pub struct ModuleGraph {
    /// All modules by name
    pub modules: HashMap<String, ModuleInfo>,
    /// Edges: module -> modules it depends on
    pub dependencies: HashMap<String, Vec<String>>,
}

impl ModuleGraph {
    /// Create a new empty module graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a module to the graph.
    pub fn add_module(&mut self, info: ModuleInfo) {
        let name = info.name.clone();
        let deps = info.imports.clone();
        self.modules.insert(name.clone(), info);
        self.dependencies.insert(name, deps);
    }

    /// Keep only the modules reachable from `roots` through imports: the
    /// modules a `.cabal` file declares and what they import. Everything under
    /// a source directory was compiled before, so a package whose source
    /// directory is its root also compiled its `Setup.hs` and its tests. No
    /// roots keeps every module.
    pub fn restrict_to(&mut self, roots: &[String]) {
        if roots.is_empty() {
            return;
        }
        let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stack: Vec<String> = roots
            .iter()
            .filter(|r| self.modules.contains_key(*r))
            .cloned()
            .collect();
        if stack.is_empty() {
            return;
        }
        while let Some(name) = stack.pop() {
            if !keep.insert(name.clone()) {
                continue;
            }
            if let Some(deps) = self.dependencies.get(&name) {
                stack.extend(
                    deps.iter()
                        .filter(|d| self.modules.contains_key(*d) && !keep.contains(*d))
                        .cloned(),
                );
            }
        }
        self.modules.retain(|name, _| keep.contains(name));
        self.dependencies.retain(|name, _| keep.contains(name));
    }

    /// Get modules in topological order (dependencies first).
    pub fn topological_order(&self) -> Result<Vec<String>, ModuleError> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut in_progress = HashSet::new();

        for name in self.modules.keys() {
            self.visit_module(name, &mut visited, &mut in_progress, &mut result)?;
        }

        Ok(result)
    }

    fn visit_module(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        in_progress: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<(), ModuleError> {
        if visited.contains(name) {
            return Ok(());
        }

        if in_progress.contains(name) {
            return Err(ModuleError::CycleDetected(name.to_string()));
        }

        in_progress.insert(name.to_string());

        if let Some(deps) = self.dependencies.get(name) {
            for dep in deps {
                // Only visit local modules
                if self.modules.contains_key(dep) {
                    self.visit_module(dep, visited, in_progress, result)?;
                }
            }
        }

        in_progress.remove(name);
        visited.insert(name.to_string());
        result.push(name.to_string());

        Ok(())
    }

    /// Get modules that can be compiled in parallel (same depth level).
    pub fn parallel_groups(&self) -> Result<Vec<Vec<String>>, ModuleError> {
        let order = self.topological_order()?;
        let mut groups: Vec<Vec<String>> = Vec::new();
        let mut compiled: HashSet<String> = HashSet::new();

        while compiled.len() < order.len() {
            let mut group = Vec::new();

            for name in &order {
                if compiled.contains(name) {
                    continue;
                }

                // Check if all local dependencies are compiled
                let deps_ready = self
                    .dependencies
                    .get(name)
                    .map(|deps| {
                        deps.iter()
                            .filter(|d| self.modules.contains_key(*d))
                            .all(|d| compiled.contains(d))
                    })
                    .unwrap_or(true);

                if deps_ready {
                    group.push(name.clone());
                }
            }

            if group.is_empty() {
                break; // Should not happen if topological order is correct
            }

            for name in &group {
                compiled.insert(name.clone());
            }

            groups.push(group);
        }

        Ok(groups)
    }
}

/// Parse import statements from a Haskell source file.
pub fn parse_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let source = strip_block_comments(source);

    for line in source.lines() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("--") {
            continue;
        }

        // Parse import statement
        if trimmed.starts_with("import")
            && let Some(module_name) = extract_module_name(trimmed)
        {
            imports.push(module_name);
        }

        // Stop at module body (simplistic heuristic)
        if trimmed.starts_with("main ") || trimmed.starts_with("main=") {
            break;
        }
    }

    imports
}

/// `source` with its `{- ... -}` block comments (nested, and pragmas) blanked
/// out, line breaks kept. A Haddock example in a block comment can show
/// `import` lines; read as the module's own, transformers-compat's
/// `Data.Functor.Classes.Generic` imported itself and the build failed with a
/// dependency cycle. Line comments (`--`) are left for the caller, so a `{-`
/// inside one does not open a block.
fn strip_block_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut depth = 0usize;
    for line in source.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let pair = (chars[i], chars.get(i + 1).copied());
            if depth == 0 && pair == ('-', Some('-')) {
                // A line comment: the rest of the line, unchanged.
                out.extend(&chars[i..]);
                break;
            }
            if pair == ('{', Some('-')) {
                depth += 1;
                i += 2;
            } else if depth > 0 && pair == ('-', Some('}')) {
                depth -= 1;
                i += 2;
            } else {
                if depth == 0 {
                    out.push(chars[i]);
                }
                i += 1;
            }
        }
        out.push('\n');
    }
    out
}

/// Extract module name from an import line.
fn extract_module_name(line: &str) -> Option<String> {
    // Handle: import Module
    // Handle: import qualified Module
    // Handle: import Module (...)
    // Handle: import Module hiding (...)
    // Handle: import Module as Alias

    let parts: Vec<&str> = line.split_whitespace().collect();

    let mut idx = 0;
    if parts.get(idx) == Some(&"import") {
        idx += 1;
    } else {
        return None;
    }

    // Skip "qualified"
    if parts.get(idx) == Some(&"qualified") {
        idx += 1;
    }

    // Get module name (stop at 'as', 'hiding', or '(')
    if let Some(name) = parts.get(idx) {
        let name = name.trim_end_matches('(');
        if !name.is_empty()
            && name
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            return Some(name.to_string());
        }
    }

    None
}

/// Scan a directory for Haskell source files.
pub fn find_haskell_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    find_haskell_files_recursive(dir, &mut files);
    files
}

fn find_haskell_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                find_haskell_files_recursive(&path, files);
            } else if path.extension().map(|e| e == "hs").unwrap_or(false) {
                files.push(path);
            }
        }
    }
}

/// Convert a file path to a module name.
pub fn path_to_module_name(base_dir: &Path, file_path: &Path) -> Option<String> {
    let relative = file_path.strip_prefix(base_dir).ok()?;
    let stem = relative.with_extension("");
    let module_name = stem
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join(".");

    if module_name.is_empty() {
        None
    } else {
        Some(module_name)
    }
}

/// Build a module graph from source directories.
pub fn build_module_graph(src_dirs: &[PathBuf]) -> Result<ModuleGraph, ModuleError> {
    let mut graph = ModuleGraph::new();

    for src_dir in src_dirs {
        let files = find_haskell_files(src_dir);

        for file_path in files {
            let content = fs::read_to_string(&file_path)?;
            let imports = parse_imports(&content);

            if let Some(module_name) = path_to_module_name(src_dir, &file_path) {
                let info = ModuleInfo {
                    name: module_name,
                    path: file_path,
                    imports,
                    is_local: true,
                };
                graph.add_module(info);
            }
        }
    }

    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_imports_skips_block_comments() {
        let source = r#"module Data.Functor.Classes.Generic where
import Data.Functor.Classes.Generic.Internal
{- $example
@
import Data.Functor.Classes
import Data.Functor.Classes.Generic
{- nested -}
import GHC.Generics
@
-}
{-# LANGUAGE CPP #-} import Data.Char
-- {- not a block
import Data.List
"#;
        assert_eq!(
            parse_imports(source),
            vec![
                "Data.Functor.Classes.Generic.Internal".to_string(),
                "Data.Char".to_string(),
                "Data.List".to_string(),
            ]
        );
    }

    #[test]
    fn test_restrict_to_reachable_modules() {
        let mut graph = ModuleGraph::new();
        for (name, imports) in [
            ("Data.A", vec!["Data.B"]),
            ("Data.B", vec!["Data.List"]),
            ("Setup", vec!["Distribution.Simple"]),
            ("Test.A", vec!["Data.A"]),
        ] {
            graph.add_module(ModuleInfo {
                name: name.into(),
                path: PathBuf::from(format!("{name}.hs")),
                imports: imports.into_iter().map(String::from).collect(),
                is_local: true,
            });
        }
        graph.restrict_to(&["Data.A".to_string()]);
        let mut names: Vec<_> = graph.modules.keys().cloned().collect();
        names.sort();
        assert_eq!(names, vec!["Data.A", "Data.B"]);
    }

    #[test]
    fn test_parse_imports_simple() {
        let source = r#"
module Main where

import Data.List
import qualified Data.Map as M
import Control.Monad (when)

main :: IO ()
main = return ()
"#;

        let imports = parse_imports(source);
        assert!(imports.contains(&"Data.List".to_string()));
        assert!(imports.contains(&"Data.Map".to_string()));
        assert!(imports.contains(&"Control.Monad".to_string()));
    }

    #[test]
    fn test_extract_module_name() {
        assert_eq!(
            extract_module_name("import Data.List"),
            Some("Data.List".to_string())
        );
        assert_eq!(
            extract_module_name("import qualified Data.Map as M"),
            Some("Data.Map".to_string())
        );
        assert_eq!(
            extract_module_name("import Control.Monad (when, unless)"),
            Some("Control.Monad".to_string())
        );
        assert_eq!(
            extract_module_name("import Data.Text hiding (head)"),
            Some("Data.Text".to_string())
        );
    }

    #[test]
    fn test_module_graph_topological() {
        let mut graph = ModuleGraph::new();

        // A depends on B, B depends on C
        graph.add_module(ModuleInfo {
            name: "A".to_string(),
            path: PathBuf::from("A.hs"),
            imports: vec!["B".to_string()],
            is_local: true,
        });
        graph.add_module(ModuleInfo {
            name: "B".to_string(),
            path: PathBuf::from("B.hs"),
            imports: vec!["C".to_string()],
            is_local: true,
        });
        graph.add_module(ModuleInfo {
            name: "C".to_string(),
            path: PathBuf::from("C.hs"),
            imports: vec![],
            is_local: true,
        });

        let order = graph.topological_order().unwrap();

        // C must come before B, B must come before A
        let c_pos = order.iter().position(|x| x == "C").unwrap();
        let b_pos = order.iter().position(|x| x == "B").unwrap();
        let a_pos = order.iter().position(|x| x == "A").unwrap();

        assert!(c_pos < b_pos);
        assert!(b_pos < a_pos);
    }

    #[test]
    fn test_parallel_groups() {
        let mut graph = ModuleGraph::new();

        // A, B independent; C depends on both
        graph.add_module(ModuleInfo {
            name: "A".to_string(),
            path: PathBuf::from("A.hs"),
            imports: vec![],
            is_local: true,
        });
        graph.add_module(ModuleInfo {
            name: "B".to_string(),
            path: PathBuf::from("B.hs"),
            imports: vec![],
            is_local: true,
        });
        graph.add_module(ModuleInfo {
            name: "C".to_string(),
            path: PathBuf::from("C.hs"),
            imports: vec!["A".to_string(), "B".to_string()],
            is_local: true,
        });

        let groups = graph.parallel_groups().unwrap();

        // First group should have A and B (can compile in parallel)
        assert!(groups[0].contains(&"A".to_string()) || groups[0].contains(&"B".to_string()));
        // C should be in a later group
        assert!(groups.iter().skip(1).any(|g| g.contains(&"C".to_string())));
    }

    #[test]
    fn test_path_to_module_name() {
        let base = PathBuf::from("/project/src");
        let file = PathBuf::from("/project/src/Data/List/Extra.hs");

        assert_eq!(
            path_to_module_name(&base, &file),
            Some("Data.List.Extra".to_string())
        );
    }
}
