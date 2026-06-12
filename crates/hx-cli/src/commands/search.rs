//! Package search command implementation.

use anyhow::Result;
use hx_solver::index::{IndexOptions, load_index};
use hx_solver::package::PackageIndex;
use hx_ui::Output;
use std::path::PathBuf;

/// Run the search command.
pub async fn run(query: &str, limit: usize, detailed: bool, output: &Output) -> Result<i32> {
    output.status("Searching", &format!("for '{}'", query));

    // Find and load the package index
    let index_path = find_index_path();
    let index = match index_path {
        Some(path) => {
            let options = IndexOptions {
                show_progress: false,
                ..Default::default()
            };
            match load_index(&path, &options) {
                Ok(idx) => idx,
                Err(e) => {
                    output.warn(&format!("Could not load package index: {}", e));
                    output.info("Run `hx index update` to download the package index");
                    return Ok(1);
                }
            }
        }
        None => {
            output.warn("Package index not found");
            output.info("Run `hx index update` to download the package index");
            return Ok(1);
        }
    };

    // Search for packages
    let results = search_packages(&index, query, limit);

    if results.is_empty() {
        output.warn(&format!("No packages found matching '{}'", query));
        return Ok(0);
    }

    output.header(&format!("Found {} package(s)", results.len()));

    for result in &results {
        if detailed {
            display_detailed_result(result, output);
        } else {
            display_brief_result(result, output);
        }
    }

    if results.len() == limit {
        output.info(&format!(
            "Showing first {} results. Use --limit to see more.",
            limit
        ));
    }

    Ok(0)
}

/// Find the package index path.
fn find_index_path() -> Option<PathBuf> {
    // Try hx-managed index first
    if let Some(cache_dir) = dirs::cache_dir() {
        let hx_index = cache_dir.join("hx").join("index").join("01-index.tar.gz");
        if hx_index.exists() {
            return Some(hx_index);
        }
    }

    // Try cabal index
    if let Some(home) = dirs::home_dir() {
        let cabal_index = home
            .join(".cabal")
            .join("packages")
            .join("hackage.haskell.org")
            .join("01-index.tar.gz");
        if cabal_index.exists() {
            return Some(cabal_index);
        }
    }

    None
}

/// A search result with package info.
#[derive(Debug)]
struct SearchResult {
    name: String,
    version: String,
}

/// Search for packages matching a query.
fn search_packages(index: &PackageIndex, query: &str, limit: usize) -> Vec<SearchResult> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    // Get all packages from index
    for (name, package) in &index.packages {
        let name_lower = name.to_lowercase();

        // Match by name (prefix, contains, or fuzzy)
        let score = compute_match_score(&name_lower, &query_lower);

        if score > 0 {
            // Get latest version
            let versions = package.versions_descending();
            if let Some(latest) = versions.first() {
                results.push((
                    score,
                    SearchResult {
                        name: name.clone(),
                        version: latest.to_string(),
                    },
                ));
            }
        }
    }

    // Sort by score (descending)
    results.sort_by_key(|r| std::cmp::Reverse(r.0));

    // Take top results
    results
        .into_iter()
        .take(limit)
        .map(|(_, info)| info)
        .collect()
}

/// Compute a match score for a package name against a query.
fn compute_match_score(name: &str, query: &str) -> u32 {
    // Exact match
    if name == query {
        return 100;
    }

    // Prefix match
    if name.starts_with(query) {
        return 80;
    }

    // Contains match
    if name.contains(query) {
        return 60;
    }

    // Word boundary match (e.g., "json" matches "aeson-json")
    let words: Vec<&str> = name.split('-').collect();
    for word in &words {
        if word.starts_with(query) {
            return 40;
        }
    }

    // Fuzzy match (simple character containment)
    let mut query_chars = query.chars().peekable();
    for c in name.chars() {
        if query_chars.peek() == Some(&c) {
            query_chars.next();
        }
    }
    if query_chars.peek().is_none() {
        return 20;
    }

    0
}

/// Display a brief search result.
fn display_brief_result(result: &SearchResult, output: &Output) {
    output.list_item(&result.name, &result.version);
}

/// Display a detailed search result.
fn display_detailed_result(result: &SearchResult, output: &Output) {
    println!();
    output.header(&format!("{} {}", result.name, result.version));
    output.info(&format!(
        "Hackage: https://hackage.haskell.org/package/{}",
        result.name
    ));
    output.info(&format!("Install: hx add {}", result.name));
}
