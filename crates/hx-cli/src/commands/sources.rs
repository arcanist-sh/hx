//! Source hashing and verification shared by `hx lock`, `hx fetch` and
//! `hx sync`.
//!
//! The lockfile pins every Hackage sdist by SHA-256 and every `.cabal` file by
//! its Hackage revision. This module records those pins (from the Hackage
//! index, which carries signed sdist hashes and every `.cabal` revision) and
//! checks cached tarballs against them.

use anyhow::{Context, Result};
use hx_lock::{LockedPackage, Lockfile};
use hx_solver::{SeedOutcome, SourceSpec, SourceStatus, check_source_file, scan_index_metadata};
use hx_ui::Output;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Source spec (name, version, pinned hash) for every sdist a build may need.
pub(crate) fn source_specs(lock: &Lockfile) -> Vec<SourceSpec> {
    lock.fetchable_sdists()
        .into_iter()
        .map(|p| SourceSpec {
            name: p.name.clone(),
            version: p.version.clone(),
            sha256: p.sha256.clone(),
        })
        .collect()
}

/// Parse a lockfile `index_state` into a Unix timestamp.
pub(crate) fn index_state_timestamp(index_state: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(index_state)
        .ok()
        .map(|t| t.timestamp())
}

/// Outcome of [`record_source_hashes`].
#[derive(Debug, Default)]
pub(crate) struct HashRecording {
    /// Packages that now carry an sdist hash.
    pub hashed: usize,
    /// Packages the index had no entry for (`name-version`).
    pub missing: Vec<String>,
}

/// Fill in `sha256`, `revision` and `cabal_sha256` for every locked sdist,
/// plus `plan.index_state`, from the Hackage index at `index_path`.
///
/// When `index_state` is given (e.g. from a cabal freeze file) the index is
/// read as of that state, so the recorded revisions are the ones Cabal will
/// use; otherwise the newest entry of the index defines the state.
pub(crate) fn record_source_hashes(
    lock: &mut Lockfile,
    index_path: &Path,
    index_state: Option<String>,
) -> Result<HashRecording> {
    let wanted: HashSet<(String, String)> = lock
        .fetchable_sdists()
        .iter()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    let cutoff = index_state.as_deref().and_then(index_state_timestamp);
    let meta = scan_index_metadata(index_path, &wanted, cutoff)
        .with_context(|| format!("Failed to read Hackage index {}", index_path.display()))?;

    let mut recording = HashRecording::default();
    for pkg in lock.packages.iter_mut().filter(|p| p.is_fetchable_sdist()) {
        match meta.get(&pkg.name, &pkg.version) {
            Some(m) => {
                if let Some(ref sha) = m.sdist_sha256 {
                    pkg.sha256 = Some(sha.clone());
                    recording.hashed += 1;
                } else {
                    recording.missing.push(pkg.package_id());
                }
                if let Some((rev, sha)) = m.latest_revision() {
                    pkg.revision = Some(rev);
                    pkg.cabal_sha256 = Some(sha.to_string());
                }
            }
            None => recording.missing.push(pkg.package_id()),
        }
    }
    lock.plan.index_state = index_state.or_else(|| meta.index_state_string());
    recording.missing.sort();
    Ok(recording)
}

/// A cached tarball whose content differs from its lockfile pin.
#[derive(Debug, Clone)]
pub(crate) struct Mismatch {
    pub id: String,
    /// The cached file, or `None` for a fresh download that was discarded.
    pub path: Option<PathBuf>,
    pub expected: String,
    pub actual: String,
}

/// Readiness of cabal's download cache for an offline build.
#[derive(Debug, Default)]
pub(crate) struct CacheReport {
    /// Sdists cabal can use (verified, or unpinned in an old lockfile).
    pub ready: usize,
    /// Sdists present but without a pin to verify against.
    pub unpinned: Vec<String>,
    /// Sdists that are in neither cabal's nor hx's cache.
    pub missing: Vec<String>,
    /// Sdists whose cached content differs from the lockfile.
    pub mismatched: Vec<Mismatch>,
    /// Sdists copied from hx's verified cache into cabal's.
    pub seeded: usize,
}

/// Check that cabal-install can find every locked sdist in `cabal_repo`
/// with the pinned content, copying verified tarballs over from hx's cache
/// (`hx_cache`) when cabal lacks them.
pub(crate) fn prepare_cabal_cache(
    lock: &Lockfile,
    hx_cache: &Path,
    cabal_repo: &Path,
) -> Result<CacheReport> {
    let mut report = CacheReport::default();
    for spec in source_specs(lock) {
        let id = spec.package_id();
        let expected = spec.sha256.as_deref();
        let cabal_path =
            hx_solver::cabal_cached_tarball_path(cabal_repo, &spec.name, &spec.version);
        match check_source_file(&cabal_path, expected)? {
            SourceStatus::Verified { .. } => report.ready += 1,
            SourceStatus::Unpinned { .. } => {
                report.ready += 1;
                report.unpinned.push(id);
            }
            SourceStatus::Mismatch {
                path,
                expected,
                actual,
            } => report.mismatched.push(Mismatch {
                id,
                path: Some(path),
                expected,
                actual,
            }),
            SourceStatus::Missing { .. } => {
                let hx_path = hx_solver::cached_tarball_path(hx_cache, &spec.name, &spec.version);
                match check_source_file(&hx_path, expected)? {
                    SourceStatus::Verified { sha256, .. }
                    | SourceStatus::Unpinned { sha256, .. } => {
                        hx_solver::seed_cabal_cache(
                            &hx_path,
                            &sha256,
                            cabal_repo,
                            &spec.name,
                            &spec.version,
                        )?;
                        report.seeded += 1;
                        report.ready += 1;
                        if expected.is_none() {
                            report.unpinned.push(id);
                        }
                    }
                    SourceStatus::Mismatch {
                        path,
                        expected,
                        actual,
                    } => report.mismatched.push(Mismatch {
                        id,
                        path: Some(path),
                        expected,
                        actual,
                    }),
                    SourceStatus::Missing { .. } => report.missing.push(id),
                }
            }
        }
    }
    Ok(report)
}

/// Print a hash mismatch as a hard error with an explanation and fixes.
pub(crate) fn report_mismatch(output: &Output, m: &Mismatch) {
    output.error(&format!("hash mismatch for {}", m.id));
    output.list_item("expected", &format!("{} (hx.lock)", m.expected));
    match m.path {
        Some(ref path) => {
            output.list_item("found   ", &format!("{} ({})", m.actual, path.display()))
        }
        None => output.list_item("found   ", &format!("{} (download, discarded)", m.actual)),
    }
    output.info(
        "  The tarball differs from the one pinned when hx.lock was created. Hackage never\n  \
         changes a published tarball, so this points to a corrupted download, a tampered\n  \
         mirror or cache, or an edited lockfile. (Hackage metadata revisions change only\n  \
         the .cabal file, which hx.lock pins separately via `revision`.)",
    );
    match m.path {
        Some(ref path) => output.info(&format!(
            "fix: delete {} and run `hx fetch` to download it again",
            path.display()
        )),
        None => output
            .info("fix: check the mirror you download from (HX_HACKAGE_URL) and retry `hx fetch`"),
    }
    output.info("     or run `hx lock` if the lockfile itself is out of date");
}

/// Print the "sources missing for an offline build" error.
pub(crate) fn report_missing_offline(output: &Output, missing: &[String]) {
    output.error(&format!(
        "{} locked source package{} not available offline",
        missing.len(),
        if missing.len() == 1 { " is" } else { "s are" }
    ));
    const SHOWN: usize = 20;
    for id in missing.iter().take(SHOWN) {
        output.info(&format!("  - {}", id));
    }
    if missing.len() > SHOWN {
        output.info(&format!("  ... and {} more", missing.len() - SHOWN));
    }
    output.info("fix: Run `hx fetch` while online to download and verify them");
}

/// A problem found by [`verify_against_index`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexFinding {
    /// The index disagrees with the lockfile (hard failure).
    Mismatch(String),
    /// The index cannot confirm a pin (e.g. it is older than the lockfile).
    Unverifiable(String),
}

/// Cross-check the lockfile's sdist hashes and `.cabal` revision pins
/// against a Hackage index, read as of the lockfile's `index_state`.
pub(crate) fn verify_against_index(
    lock: &Lockfile,
    index_path: &Path,
) -> Result<Vec<IndexFinding>> {
    let pinned: Vec<&LockedPackage> = lock
        .fetchable_sdists()
        .into_iter()
        .filter(|p| p.sha256.is_some() || p.cabal_sha256.is_some())
        .collect();
    let wanted: HashSet<(String, String)> = pinned
        .iter()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    let cutoff = lock
        .plan
        .index_state
        .as_deref()
        .and_then(index_state_timestamp);
    let meta = scan_index_metadata(index_path, &wanted, cutoff)
        .with_context(|| format!("Failed to read Hackage index {}", index_path.display()))?;

    let mut findings = Vec::new();
    for pkg in pinned {
        let id = pkg.package_id();
        let Some(m) = meta.get(&pkg.name, &pkg.version) else {
            findings.push(IndexFinding::Unverifiable(format!(
                "{id} is not in the Hackage index (the index may be older than hx.lock; run `cabal update`)"
            )));
            continue;
        };
        if let (Some(locked), Some(indexed)) = (&pkg.sha256, &m.sdist_sha256)
            && locked != indexed
        {
            findings.push(IndexFinding::Mismatch(format!(
                "{id}: sdist sha256 {locked} in hx.lock, but the index records {indexed}"
            )));
        }
        if let (Some(rev), Some(locked)) = (pkg.revision, &pkg.cabal_sha256) {
            match m.cabal_revisions.get(rev as usize) {
                Some(indexed) if indexed != locked => {
                    findings.push(IndexFinding::Mismatch(format!(
                        "{id}: .cabal revision {rev} hashes to {indexed} in the index, \
                         but hx.lock pins {locked}"
                    )));
                }
                Some(_) => {}
                None => findings.push(IndexFinding::Unverifiable(format!(
                    "{id}: hx.lock pins .cabal revision {rev}, but the index only has {} \
                     (run `cabal update`)",
                    m.cabal_revisions.len()
                ))),
            }
        }
    }
    Ok(findings)
}

/// Report what [`hx_solver::seed_cabal_cache`] did, warning about replaced
/// copies (cabal had different content than the verified tarball).
pub(crate) fn note_seed(output: &Output, id: &str, outcome: SeedOutcome, cabal_repo: &Path) {
    if outcome == SeedOutcome::Replaced {
        output.warn(&format!(
            "replaced cabal's cached {} under {}: it did not match hx.lock",
            id,
            cabal_repo.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_solver::sha256_hex;
    use std::fs;

    fn lock_with(content: &[u8]) -> Lockfile {
        let mut lock = Lockfile::new();
        let mut pkg = LockedPackage::hackage("pkg", "1.0");
        pkg.sha256 = Some(sha256_hex(content));
        lock.add_package(pkg);
        lock.add_package(LockedPackage::hackage("base", "4.19.1.0"));
        lock
    }

    #[test]
    fn test_prepare_cabal_cache_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let report = prepare_cabal_cache(
            &lock_with(b"x"),
            &dir.path().join("hx"),
            &dir.path().join("cabal"),
        )
        .unwrap();
        // `base` is compiler-internal and never needs an sdist.
        assert_eq!(report.missing, vec!["pkg-1.0".to_string()]);
        assert_eq!(report.ready, 0);
    }

    #[test]
    fn test_prepare_cabal_cache_seeds_from_hx_cache() {
        let dir = tempfile::tempdir().unwrap();
        let hx = dir.path().join("hx");
        let cabal = dir.path().join("cabal");
        fs::create_dir_all(&hx).unwrap();
        fs::write(hx.join("pkg-1.0.tar.gz"), b"sdist").unwrap();

        let report = prepare_cabal_cache(&lock_with(b"sdist"), &hx, &cabal).unwrap();
        assert_eq!((report.ready, report.seeded), (1, 1));
        assert!(cabal.join("pkg/1.0/pkg-1.0.tar.gz").is_file());

        // Already present now: nothing more to seed.
        let report = prepare_cabal_cache(&lock_with(b"sdist"), &hx, &cabal).unwrap();
        assert_eq!((report.ready, report.seeded), (1, 0));
    }

    #[test]
    fn test_prepare_cabal_cache_flags_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let cabal = dir.path().join("cabal");
        let path = cabal.join("pkg/1.0/pkg-1.0.tar.gz");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"tampered").unwrap();

        let report =
            prepare_cabal_cache(&lock_with(b"sdist"), &dir.path().join("hx"), &cabal).unwrap();
        assert_eq!(report.mismatched.len(), 1);
        let m = &report.mismatched[0];
        assert_eq!(m.id, "pkg-1.0");
        assert_eq!(m.path.as_deref(), Some(path.as_path()));
        assert_eq!(m.actual, sha256_hex(b"tampered"));
        assert_eq!(m.expected, sha256_hex(b"sdist"));
    }

    /// Write a gzip-free index tar with one package version.
    fn write_index(dir: &Path, cabal_revisions: &[&[u8]], sdist_sha: &str) -> PathBuf {
        let mut builder = tar::Builder::new(Vec::new());
        let mut append = |path: &str, content: &[u8], mtime: u64| {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(mtime);
            header.set_cksum();
            builder.append(&header, content).unwrap();
        };
        for (i, rev) in cabal_revisions.iter().enumerate() {
            append("pkg/1.0/pkg.cabal", rev, 1_000 + i as u64);
        }
        let json = format!(
            r#"{{"signed":{{"targets":{{"<repo>/package/pkg-1.0.tar.gz":{{"hashes":{{"sha256":"{sdist_sha}"}},"length":5}}}}}}}}"#
        );
        append("pkg/1.0/package.json", json.as_bytes(), 1_000);
        let path = dir.join("01-index.tar");
        fs::write(&path, builder.into_inner().unwrap()).unwrap();
        path
    }

    #[test]
    fn test_record_and_verify_against_index() {
        let dir = tempfile::tempdir().unwrap();
        let index = write_index(dir.path(), &[b"rev0", b"rev1"], &sha256_hex(b"sdist"));

        let mut lock = Lockfile::new();
        lock.add_package(LockedPackage::hackage("pkg", "1.0"));
        lock.add_package(LockedPackage::hackage("gone", "0.1"));
        let rec = record_source_hashes(&mut lock, &index, None).unwrap();
        assert_eq!(rec.hashed, 1);
        assert_eq!(rec.missing, vec!["gone-0.1".to_string()]);
        let pkg = lock.package("pkg").unwrap();
        assert_eq!(pkg.sha256, Some(sha256_hex(b"sdist")));
        assert_eq!(pkg.revision, Some(1));
        assert_eq!(pkg.cabal_sha256, Some(sha256_hex(b"rev1")));
        assert_eq!(
            lock.plan.index_state.as_deref(),
            Some("1970-01-01T00:16:41Z")
        );

        // The lockfile agrees with the index it was made from.
        assert!(verify_against_index(&lock, &index).unwrap().is_empty());

        // A tampered revision pin is a mismatch.
        let mut tampered = lock.clone();
        tampered.packages.iter_mut().for_each(|p| {
            if p.name == "pkg" {
                p.cabal_sha256 = Some(sha256_hex(b"evil"));
            }
        });
        let findings = verify_against_index(&tampered, &index).unwrap();
        assert!(matches!(&findings[0], IndexFinding::Mismatch(m) if m.contains("revision 1")));
    }

    #[test]
    fn test_record_honours_freeze_index_state() {
        let dir = tempfile::tempdir().unwrap();
        let index = write_index(dir.path(), &[b"rev0", b"rev1"], &sha256_hex(b"sdist"));
        let mut lock = Lockfile::new();
        lock.add_package(LockedPackage::hackage("pkg", "1.0"));
        // At t=1000 only revision 0 had been published.
        record_source_hashes(&mut lock, &index, Some("1970-01-01T00:16:40Z".into())).unwrap();
        let pkg = lock.package("pkg").unwrap();
        assert_eq!(pkg.revision, Some(0));
        assert_eq!(pkg.cabal_sha256, Some(sha256_hex(b"rev0")));
        assert_eq!(
            lock.plan.index_state.as_deref(),
            Some("1970-01-01T00:16:40Z")
        );
    }
}
