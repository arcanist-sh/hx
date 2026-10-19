//! Cache management command implementation.

use anyhow::Result;
use hx_cache::{
    ArtifactIndex, HxPaths, StoreIndex, clean_global_cache, clear_artifacts, global_cache_dir,
    prune_artifacts, store_disk_size,
};
use hx_ui::Output;

/// Show cache statistics.
pub async fn status(output: &Output) -> Result<i32> {
    output.header("Cache Status");

    // Load store index
    let store = StoreIndex::load().unwrap_or_default();
    let stats = store.stats();

    output.list_item("cached builds", &stats.entry_count.to_string());
    output.list_item("total packages", &stats.total_packages.to_string());

    // Show GHC version breakdown
    if !stats.ghc_versions.is_empty() {
        output.header("By GHC Version");
        let mut versions: Vec<_> = stats.ghc_versions.iter().collect();
        versions.sort_by(|a, b| b.1.cmp(a.1));
        for (version, count) in versions {
            output.list_item(version, &format!("{} builds", count));
        }
    }

    // Show where the caches live (`hx config path --all` lists everything)
    if let Ok(paths) = HxPaths::current() {
        output.header("Locations");
        output.list_item("cache", &paths.cache.path.display().to_string());
        output.list_item("cabal store", &paths.cabal_store.path.display().to_string());
        // Older-layout caches outside the cache root, if present
        for root in paths.cache_roots().iter().skip(1).filter(|r| r.exists()) {
            output.list_item("also", &root.display().to_string());
        }
    }

    // Show disk usage
    match store_disk_size() {
        Ok(size) => {
            output.header("Disk Usage");
            output.list_item("store size", &format_bytes(size));
        }
        Err(e) => {
            output.verbose(&format!("Could not determine store size: {}", e));
        }
    }

    Ok(0)
}

/// Prune old cache entries.
pub async fn prune(days: u64, output: &Output) -> Result<i32> {
    let max_age_secs = days * 24 * 60 * 60;

    let mut store = StoreIndex::load().unwrap_or_default();
    let before = store.stats().entry_count;
    let pruned = store.prune_older_than(max_age_secs);

    if pruned > 0 {
        store.save()?;
        output.status("Pruned", &format!("{} old cache entries", pruned));
    } else {
        output.info("No old cache entries to prune");
    }

    output.info(&format!(
        "Remaining: {} entries (was {})",
        before - pruned,
        before
    ));

    Ok(0)
}

/// Clean the entire cache.
pub async fn clean(output: &Output) -> Result<i32> {
    output.status("Cleaning", "global cache");

    clean_global_cache()?;

    output.status("Cleaned", "global cache removed");
    output.info("Run `hx lock && hx build` to rebuild dependencies");

    Ok(0)
}

/// Show artifact cache status.
pub async fn artifacts_status(output: &Output) -> Result<i32> {
    let cache_dir = global_cache_dir()?;
    let index = ArtifactIndex::load(&cache_dir).unwrap_or_default();
    let stats = index.stats();

    output.header("Artifact Cache Status");
    output.list_item("cached modules", &stats.entry_count.to_string());
    output.list_item("total size", &stats.size_string());

    if stats.entry_count > 0 {
        output.info("");
        output.info("Use `hx cache artifacts prune` to remove old artifacts");
    }

    Ok(0)
}

/// Prune old artifacts.
pub async fn artifacts_prune(days: u64, output: &Output) -> Result<i32> {
    let cache_dir = global_cache_dir()?;

    output.status("Pruning", &format!("artifacts older than {} days", days));

    let result = prune_artifacts(&cache_dir, days)?;

    if result.removed_count > 0 {
        output.status(
            "Pruned",
            &format!(
                "{} artifacts ({})",
                result.removed_count,
                format_bytes(result.removed_size)
            ),
        );
    } else {
        output.info("No old artifacts to prune");
    }

    Ok(0)
}

/// Clear all artifacts.
pub async fn artifacts_clear(output: &Output) -> Result<i32> {
    let cache_dir = global_cache_dir()?;

    output.status("Clearing", "artifact cache");
    clear_artifacts(&cache_dir)?;
    output.status("Cleared", "all artifacts removed");

    Ok(0)
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
