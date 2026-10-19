//! `hx install` / `hx uninstall`: build Haskell executables and put them on
//! PATH in `~/.hx/bin`, like `cargo install`.
//!
//! The build itself is delegated to `cabal install --install-method=copy`
//! into a private staging directory, using hx's resolved GHC and Cabal. hx
//! then moves the executables into the bin directory itself, so it can detect
//! name collisions first and record exactly what it placed in
//! `~/.hx/installed.toml` (see [`super::installed`]).

use super::installed::{Collision, InstallSource, InstalledManifest, InstalledPackage, Owner};
use anyhow::Result;
use chrono::Utc;
use hx_cabal::install::{self as cabal_install, InstallOptions};
use hx_cache::{hx_home, installed_manifest_path, toolchain_bin_dir};
use hx_core::{CommandRunner, Error, Fix};
use hx_solver::Version;
use hx_toolchain::{
    RECOMMENDED_CABAL_VERSION, ResolutionConfig, ToolStatus, Toolchain, resolve_ghc,
};
use hx_ui::{Output, Spinner};
use std::path::{Path, PathBuf};

/// Everything `hx install` was asked to do.
#[derive(Debug, Clone, Default)]
pub struct InstallRequest {
    /// `name`, `name@version` or `name-version`; with `--git`/`--path`, the
    /// package to pick from a multi-package project.
    pub package: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub path: Option<PathBuf>,
    pub ghc: Option<String>,
    pub jobs: Option<usize>,
    pub force: bool,
    pub list: bool,
}

/// A Hackage package name with an optional exact version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub version: Option<String>,
}

/// Parse `name`, `name@1.2.3` or `name-1.2.3`.
///
/// The `name-1.2.3` form is unambiguous because a Cabal package name
/// component can never be purely numeric.
pub fn parse_package_spec(spec: &str) -> std::result::Result<PackageSpec, String> {
    let spec = spec.trim();
    let (name, version) = match spec.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => match spec.rsplit_once('-') {
            Some((name, last)) if is_version(last) => (name, Some(last)),
            _ => (spec, None),
        },
    };

    if !is_package_name(name) {
        return Err(format!("`{spec}` is not a valid package name"));
    }
    if let Some(version) = version
        && !is_version(version)
    {
        return Err(format!(
            "`{version}` is not a valid version (expected e.g. 1.2.3)"
        ));
    }

    Ok(PackageSpec {
        name: name.to_string(),
        version: version.map(str::to_string),
    })
}

fn is_version(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_package_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|component| {
            !component.is_empty()
                && component.chars().all(|c| c.is_ascii_alphanumeric())
                && !component.chars().all(|c| c.is_ascii_digit())
        })
}

/// Where the package to build comes from, after argument validation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Hackage(PackageSpec),
    Git {
        url: String,
        rev: Option<String>,
        package: Option<String>,
    },
    Path {
        path: PathBuf,
        package: Option<String>,
    },
}

fn source_from_request(req: &InstallRequest) -> std::result::Result<Source, String> {
    match (&req.git, &req.path, &req.package) {
        (Some(_), Some(_), _) => Err("--git and --path cannot be used together".to_string()),
        (Some(url), None, package) => Ok(Source::Git {
            url: url.clone(),
            rev: req.rev.clone(),
            package: package.clone(),
        }),
        (None, Some(path), package) => Ok(Source::Path {
            path: path.clone(),
            package: package.clone(),
        }),
        (None, None, Some(spec)) => parse_package_spec(spec).map(Source::Hackage),
        (None, None, None) => {
            Err("specify a package, `--git <url>`, `--path <dir>` or `--list`".to_string())
        }
    }
}

/// Run `hx install`.
pub async fn run(req: InstallRequest, output: &Output) -> Result<i32> {
    if req.list {
        return list(output);
    }

    let source = match source_from_request(&req) {
        Ok(source) => source,
        Err(msg) => {
            output.error(&msg);
            output.info("Usage: hx install <package>[@<version>] | --git <url> | --path <dir>");
            return Ok(2);
        }
    };

    let bin_dir = toolchain_bin_dir()?;
    let manifest_path = installed_manifest_path()?;
    let mut manifest = InstalledManifest::load(&manifest_path)?;

    // Hackage: pin the exact version up front so the manifest records what
    // was built and an up-to-date install can be skipped.
    if let Source::Hackage(spec) = &source {
        let version = match &spec.version {
            Some(v) => v.clone(),
            None => match resolve_latest_version(&spec.name, output).await {
                Ok(version) => version,
                Err(unresolved) => {
                    output.error(&unresolved.message);
                    for fix in &unresolved.fixes {
                        output.print_fix(fix);
                    }
                    return Ok(1);
                }
            },
        };
        if !req.force
            && let Some(existing) = manifest.packages.get(&spec.name)
            && existing.source == InstallSource::Hackage
            && existing.version == version
            && existing
                .executables
                .iter()
                .all(|e| bin_dir.join(e).exists())
        {
            output.status(
                "Fresh",
                &format!("{} {} is already installed", spec.name, version),
            );
            output.info("Use `hx install --force` to rebuild it.");
            return Ok(0);
        }
        let spec = PackageSpec {
            name: spec.name.clone(),
            version: Some(version),
        };
        return install(
            Source::Hackage(spec),
            &req,
            &bin_dir,
            &manifest_path,
            &mut manifest,
            output,
        )
        .await;
    }

    install(
        source,
        &req,
        &bin_dir,
        &manifest_path,
        &mut manifest,
        output,
    )
    .await
}

/// A source checkout ready to hand to cabal.
struct Prepared {
    name: String,
    version: String,
    working_dir: PathBuf,
    target: Option<String>,
    source: InstallSource,
    /// Directory the GHC version hint (`hx.toml`) is read from.
    project_dir: Option<PathBuf>,
    // Keep temporary directories alive until the build finishes.
    _guards: Vec<tempfile::TempDir>,
}

async fn install(
    source: Source,
    req: &InstallRequest,
    bin_dir: &Path,
    manifest_path: &Path,
    manifest: &mut InstalledManifest,
    output: &Output,
) -> Result<i32> {
    let prepared = match prepare_source(source, output).await? {
        Ok(prepared) => prepared,
        Err(code) => return Ok(code),
    };

    // A local project's hx.toml may pin its GHC; an explicit --ghc wins.
    let ghc_request = req.ghc.clone().or_else(|| {
        prepared
            .project_dir
            .as_deref()
            .and_then(|dir| hx_config::Project::load(dir).ok())
            .and_then(|p| p.manifest.toolchain.ghc)
    });
    let toolchain = match resolve_build_toolchain(ghc_request.as_deref()).await {
        Ok(tc) => tc,
        Err(e) => {
            output.print_error(&e);
            return Ok(e.code().exit_code());
        }
    };

    output.status(
        "Installing",
        &format!("{} {}", prepared.name, prepared.version),
    );

    // Stage inside the hx home so the final move into bin is a same-filesystem
    // rename: an executable in bin is either the old file or the new one.
    let home = hx_home()?;
    std::fs::create_dir_all(&home)?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&home)?;

    let options = InstallOptions {
        cabal: toolchain.cabal.clone(),
        working_dir: prepared.working_dir.clone(),
        target: prepared.target.clone(),
        install_dir: staging.path().to_path_buf(),
        ghc: Some(toolchain.ghc.clone()),
        jobs: req.jobs,
        verbose: output.is_verbose(),
        toolchain_bin_dirs: toolchain.bin_dirs.clone(),
    };

    if let Err(e) = cabal_install::install(&options, output).await {
        let mut e = e;
        if let Error::BuildFailed { fixes, .. } = &mut e {
            fixes.push(Fix::with_command(
                "See the full cabal output",
                format!("hx install {} --verbose", install_hint(&prepared, req)),
            ));
            let pinned = req
                .package
                .as_deref()
                .and_then(|p| parse_package_spec(p).ok())
                .is_some_and(|spec| spec.version.is_some());
            if prepared.source == InstallSource::Hackage && !pinned {
                fixes.push(Fix::new(format!(
                    "If the latest {} does not support GHC {}, pin an older version with \
                     `hx install {}@<version>` or pick a compiler with `--ghc`",
                    prepared.name, toolchain.ghc_version, prepared.name
                )));
            }
        }
        output.print_error(&e);
        return Ok(e.code().exit_code());
    }

    let executables = staged_executables(staging.path())?;
    if executables.is_empty() {
        output.error(&format!(
            "{} {} has no executables to install",
            prepared.name, prepared.version
        ));
        output.info("`hx install` installs programs; to use a library, `hx add` it instead.");
        return Ok(1);
    }

    let collisions = manifest.collisions(&prepared.name, &executables, bin_dir);
    if !collisions.is_empty() && !req.force {
        report_collisions(&prepared.name, &collisions, bin_dir, output);
        return Ok(1);
    }
    for collision in &collisions {
        if let Some(previous) = manifest.release(&collision.executable) {
            output.warn(&format!(
                "Replacing `{}` previously installed by {}",
                collision.executable, previous
            ));
        } else {
            output.warn(&format!(
                "Overwriting {}",
                bin_dir.join(&collision.executable).display()
            ));
        }
    }

    std::fs::create_dir_all(bin_dir)?;
    for exe in &executables {
        place_executable(&staging.path().join(exe), &bin_dir.join(exe))?;
    }

    // Executables the previous install of this package had but this one does
    // not (renamed or dropped upstream) would otherwise be orphaned.
    if let Some(previous) = manifest.packages.get(&prepared.name) {
        for stale in previous
            .executables
            .iter()
            .filter(|e| !executables.contains(e))
        {
            remove_file_if_present(&bin_dir.join(stale))?;
        }
    }

    manifest.packages.insert(
        prepared.name.clone(),
        InstalledPackage {
            version: prepared.version.clone(),
            executables: executables.clone(),
            ghc: Some(toolchain.ghc_version.clone()),
            source: prepared.source.clone(),
            installed_at: Utc::now(),
        },
    );
    manifest.save(manifest_path)?;

    output.status(
        "Installed",
        &format!(
            "{} {} ({}) to {}",
            prepared.name,
            prepared.version,
            executables.join(", "),
            bin_dir.display()
        ),
    );
    warn_if_not_on_path(bin_dir, output);

    Ok(0)
}

/// The argument a user would pass to `hx install` to retry this install.
fn install_hint(prepared: &Prepared, req: &InstallRequest) -> String {
    match &prepared.source {
        InstallSource::Hackage => format!("{}@{}", prepared.name, prepared.version),
        InstallSource::Git { url, .. } => match &req.rev {
            Some(rev) => format!("--git {url} --rev {rev}"),
            None => format!("--git {url}"),
        },
        InstallSource::Path { path } => format!("--path {}", path.display()),
    }
}

/// Fetch or locate the source. `Ok(Err(code))` means the problem was already
/// reported and the command should exit with `code`.
async fn prepare_source(
    source: Source,
    output: &Output,
) -> Result<std::result::Result<Prepared, i32>> {
    match source {
        Source::Hackage(spec) => {
            let version = spec.version.expect("Hackage version is resolved first");
            // An empty directory: cabal must not pick up a cabal.project from
            // wherever the user happens to run `hx install`.
            let work = tempfile::Builder::new().prefix("hx-install-").tempdir()?;
            Ok(Ok(Prepared {
                target: Some(format!("{}-{}", spec.name, version)),
                name: spec.name,
                version,
                working_dir: work.path().to_path_buf(),
                source: InstallSource::Hackage,
                project_dir: None,
                _guards: vec![work],
            }))
        }
        Source::Path { path, package } => {
            if !path.is_dir() {
                output.error(&format!("{} is not a directory", path.display()));
                return Ok(Err(2));
            }
            let path = std::fs::canonicalize(&path)?;
            let Some((name, version)) = local_package(&path, package.as_deref(), output) else {
                return Ok(Err(1));
            };
            Ok(Ok(Prepared {
                target: Some(name.clone()),
                name,
                version,
                working_dir: path.clone(),
                source: InstallSource::Path { path: path.clone() },
                project_dir: Some(path),
                _guards: vec![],
            }))
        }
        Source::Git { url, rev, package } => {
            let checkout = tempfile::Builder::new()
                .prefix("hx-install-git-")
                .tempdir()?;
            let commit = match clone_repo(&url, rev.as_deref(), checkout.path(), output).await {
                Ok(commit) => commit,
                Err(e) => {
                    output.print_error(&e);
                    return Ok(Err(e.code().exit_code()));
                }
            };
            let Some((name, version)) = local_package(checkout.path(), package.as_deref(), output)
            else {
                return Ok(Err(1));
            };
            Ok(Ok(Prepared {
                target: Some(name.clone()),
                name,
                version,
                working_dir: checkout.path().to_path_buf(),
                source: InstallSource::Git { url, rev: commit },
                project_dir: Some(checkout.path().to_path_buf()),
                _guards: vec![checkout],
            }))
        }
    }
}

/// Find the package to install in a local project: `package` if given (in
/// the root or one directory down, as in a `cabal.project` workspace),
/// otherwise the single `.cabal` file in the root.
fn local_package(dir: &Path, package: Option<&str>, output: &Output) -> Option<(String, String)> {
    let mut candidates = cabal_files(dir);
    if package.is_some() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    candidates.extend(cabal_files(&entry.path()));
                }
            }
        }
    }

    let parsed: Vec<(String, String)> = candidates
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .map(|content| hx_solver::parse_cabal(&content))
        .filter(|cabal| !cabal.name.is_empty())
        .map(|cabal| (cabal.name, cabal.version))
        .collect();

    let found = match package {
        Some(wanted) => parsed.into_iter().find(|(name, _)| name == wanted),
        None if parsed.len() == 1 => parsed.into_iter().next(),
        None if parsed.is_empty() => None,
        None => {
            let names: Vec<_> = parsed.iter().map(|(n, _)| n.as_str()).collect();
            output.error(&format!(
                "{} contains several packages: {}",
                dir.display(),
                names.join(", ")
            ));
            output.info("Name the one to install, e.g. `hx install <package> --path <dir>`.");
            return None;
        }
    };

    match found {
        Some((name, version)) => {
            let version = if version.is_empty() {
                "0".to_string()
            } else {
                version
            };
            Some((name, version))
        }
        None => {
            match package {
                Some(wanted) => output.error(&format!(
                    "No package named `{wanted}` found in {}",
                    dir.display()
                )),
                None => output.error(&format!("No .cabal file found in {}", dir.display())),
            }
            None
        }
    }
}

fn cabal_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "cabal"))
        .collect();
    files.sort();
    files
}

/// Clone `url` into `dest`, check out `rev` if given, and return the commit
/// that was checked out.
async fn clone_repo(
    url: &str,
    rev: Option<&str>,
    dest: &Path,
    output: &Output,
) -> hx_core::Result<Option<String>> {
    if which::which("git").is_err() {
        return Err(Error::ToolchainMissing {
            tool: "git".to_string(),
            source: None,
            fixes: vec![Fix::new(
                "Install git (https://git-scm.com/downloads) to install from a repository",
            )],
        });
    }

    let spinner = Spinner::new(format!("Cloning {url}..."));
    let dest_str = dest.to_string_lossy().to_string();
    let runner = CommandRunner::new();

    // Without a revision a shallow clone of the default branch is enough.
    let mut args = vec!["clone", "--quiet"];
    if rev.is_none() {
        args.extend(["--depth", "1"]);
    }
    args.extend([url, dest_str.as_str()]);

    let clone = runner.run("git", args).await?;
    if !clone.success() {
        spinner.finish_error(format!("Failed to clone {url}"));
        return Err(Error::CommandFailed {
            command: format!("git clone {url}"),
            exit_code: Some(clone.exit_code),
            stdout: clone.stdout,
            stderr: clone.stderr.clone(),
            fixes: vec![Fix::new(format!(
                "Check the URL and your access to it: {}",
                clone.stderr.trim()
            ))],
        });
    }

    let in_repo = CommandRunner::new().with_working_dir(dest);
    if let Some(rev) = rev {
        let checkout = in_repo.run("git", ["checkout", "--quiet", rev]).await?;
        if !checkout.success() {
            spinner.finish_error(format!("Revision {rev} not found"));
            return Err(Error::CommandFailed {
                command: format!("git checkout {rev}"),
                exit_code: Some(checkout.exit_code),
                stdout: checkout.stdout,
                stderr: checkout.stderr,
                fixes: vec![Fix::new(format!(
                    "Check that `{rev}` is a commit, tag or branch of {url}"
                ))],
            });
        }
    }

    let head = in_repo.run("git", ["rev-parse", "HEAD"]).await?;
    let commit = head
        .success()
        .then(|| head.stdout.trim().to_string())
        .filter(|c| !c.is_empty());
    spinner.finish_success(format!("Cloned {url}"));
    output.verbose(&format!(
        "Checked out {}",
        commit.as_deref().unwrap_or("HEAD")
    ));
    Ok(commit)
}

/// The toolchain an install builds with.
struct BuildToolchain {
    ghc: PathBuf,
    ghc_version: String,
    cabal: PathBuf,
    bin_dirs: Vec<PathBuf>,
}

/// Resolve GHC (an explicit version, else the active one) and Cabal.
async fn resolve_build_toolchain(ghc_version: Option<&str>) -> hx_core::Result<BuildToolchain> {
    let detected = Toolchain::detect().await;

    let (ghc, version) = match ghc_version {
        Some(wanted) => {
            let resolved = resolve_ghc(&ResolutionConfig {
                cli_version: Some(wanted.to_string()),
                project_version: None,
            })
            .ok()
            .flatten();
            match resolved {
                Some(r) => (r.ghc_path(), r.version),
                // Not hx-managed, but the GHC on PATH may be the right one.
                None => match &detected.ghc.status {
                    ToolStatus::Found { version, path }
                        if version.to_string() == wanted
                            || version.to_string().starts_with(&format!("{wanted}.")) =>
                    {
                        (path.clone(), version.to_string())
                    }
                    _ => {
                        return Err(Error::ToolchainMissing {
                            tool: format!("GHC {wanted}"),
                            source: None,
                            fixes: vec![Fix::with_command(
                                format!("Install GHC {wanted}"),
                                format!("hx toolchain install --ghc {wanted}"),
                            )],
                        });
                    }
                },
            }
        }
        None => match &detected.ghc.status {
            ToolStatus::Found { version, path } => (path.clone(), version.to_string()),
            ToolStatus::VersionUnknown { path } => (path.clone(), "unknown".to_string()),
            ToolStatus::NotFound => return Err(Error::toolchain_missing("ghc")),
        },
    };

    let cabal = match &detected.cabal.status {
        ToolStatus::Found { path, .. } | ToolStatus::VersionUnknown { path } => path.clone(),
        // No active Cabal, but hx may have installed one without `--set`.
        ToolStatus::NotFound => match newest_installed_cabal() {
            Some(path) => path,
            None => {
                return Err(Error::ToolchainMissing {
                    tool: "cabal".to_string(),
                    source: None,
                    fixes: vec![Fix::with_command(
                        "Install Cabal",
                        format!("hx toolchain install --cabal {RECOMMENDED_CABAL_VERSION} --set"),
                    )],
                });
            }
        },
    };

    let bin_dirs = [&ghc, &cabal]
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();

    Ok(BuildToolchain {
        ghc,
        ghc_version: version,
        cabal,
        bin_dirs,
    })
}

/// The newest hx-managed Cabal that exists on disk, active or not.
fn newest_installed_cabal() -> Option<PathBuf> {
    let installed = hx_toolchain::list_installed_cabal(&hx_cache::toolchain_dir().ok()?).ok()?;
    installed
        .iter()
        .filter(|c| c.cabal_path().exists())
        .filter_map(|c| {
            c.version
                .parse::<Version>()
                .ok()
                .map(|v| (v, c.cabal_path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// Why the version to install could not be determined.
struct Unresolved {
    message: String,
    fixes: Vec<Fix>,
}

/// The latest preferred (non-deprecated) version of `name` on Hackage, falling
/// back to the local package index when Hackage cannot be reached.
async fn resolve_latest_version(
    name: &str,
    output: &Output,
) -> std::result::Result<String, Unresolved> {
    let not_found = || Unresolved {
        message: format!("package `{name}` not found on Hackage"),
        fixes: vec![Fix::with_command(
            "Search for the right name",
            format!("hx search {name}"),
        )],
    };

    match fetch_preferred_versions(name).await {
        Ok(Some(versions)) => {
            return latest_of(&versions).ok_or_else(not_found);
        }
        Ok(None) => return Err(not_found()),
        Err(e) => output.verbose(&format!(
            "Hackage lookup failed ({e}); trying the local index"
        )),
    }

    let index_versions = hx_solver::best_index_path().and_then(|path| {
        let options = hx_solver::IndexOptions {
            filter_packages: vec![name.to_string()],
            ..Default::default()
        };
        let index = hx_solver::load_index(&path, &options).ok()?;
        let pkg = index.packages.get(name)?;
        Some(
            pkg.versions_descending()
                .into_iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>(),
        )
    });

    match index_versions {
        Some(versions) => latest_of(&versions).ok_or_else(not_found),
        None => Err(Unresolved {
            message: format!("could not determine the latest version of `{name}`"),
            fixes: vec![
                Fix::new("Check your network connection to hackage.haskell.org"),
                Fix::new(format!("Or pin a version: `hx install {name}@<version>`")),
            ],
        }),
    }
}

/// Query Hackage's preferred-versions endpoint. `Ok(None)` means the package
/// does not exist.
async fn fetch_preferred_versions(name: &str) -> std::result::Result<Option<Vec<String>>, String> {
    #[derive(serde::Deserialize)]
    struct Preferred {
        #[serde(rename = "normal-version", default)]
        normal: Vec<String>,
    }

    let url = format!("https://hackage.haskell.org/package/{name}/preferred");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let preferred: Preferred = response.json().await.map_err(|e| e.to_string())?;
    Ok(Some(preferred.normal))
}

/// The highest version in `versions`, by PVP ordering.
fn latest_of(versions: &[String]) -> Option<String> {
    versions
        .iter()
        .filter_map(|v| v.parse::<Version>().ok().map(|parsed| (parsed, v)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v)| v.clone())
}

/// File names cabal copied into the staging directory.
fn staged_executables(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_file() || file_type.is_symlink() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Move a staged executable into the bin directory, replacing what is there.
fn place_executable(src: &Path, dest: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    // Different filesystem (HX_HOME/bin on another mount): copy beside the
    // destination, then rename, so the swap itself is still atomic.
    let tmp = dest.with_file_name(format!(
        ".{}.hx-tmp",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dest)
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn report_collisions(package: &str, collisions: &[Collision], bin_dir: &Path, output: &Output) {
    output.error(&format!(
        "{package} would overwrite {} in {}",
        if collisions.len() == 1 {
            "an existing executable".to_string()
        } else {
            format!("{} existing executables", collisions.len())
        },
        bin_dir.display()
    ));
    for collision in collisions {
        let owner = match &collision.owner {
            Owner::Package(pkg) => format!("installed by {pkg}"),
            Owner::Unmanaged => "not installed by `hx install`".to_string(),
        };
        eprintln!("  {} ({owner})", collision.executable);
    }
    eprintln!();
    eprintln!("fix: Re-run with `--force` to replace them");
    if let Some(Owner::Package(pkg)) = collisions
        .iter()
        .map(|c| &c.owner)
        .find(|o| matches!(o, Owner::Package(_)))
    {
        eprintln!("     or remove the other package first: `hx uninstall {pkg}`");
    }
}

fn warn_if_not_on_path(bin_dir: &Path, output: &Output) {
    let on_path = std::env::var_os("PATH").is_some_and(|path| {
        let canonical = std::fs::canonicalize(bin_dir).ok();
        std::env::split_paths(&path).any(|p| {
            p == bin_dir || (canonical.is_some() && std::fs::canonicalize(&p).ok() == canonical)
        })
    });
    if !on_path {
        output.warn(&format!("{} is not on your PATH", bin_dir.display()));
        output.info(&format!(
            "Add it to your shell profile: export PATH=\"{}:$PATH\"",
            bin_dir.display()
        ));
    }
}

/// `hx install --list`.
fn list(output: &Output) -> Result<i32> {
    let manifest = InstalledManifest::load(&installed_manifest_path()?)?;
    if manifest.packages.is_empty() {
        output.info("No packages installed with `hx install`.");
        return Ok(0);
    }
    for (name, pkg) in &manifest.packages {
        let ghc = pkg
            .ghc
            .as_deref()
            .map(|g| format!(", ghc {g}"))
            .unwrap_or_default();
        println!("{name} {} ({}{ghc}):", pkg.version, pkg.source.describe());
        for exe in &pkg.executables {
            println!("    {exe}");
        }
    }
    Ok(0)
}

/// Run `hx uninstall`.
pub async fn uninstall(packages: Vec<String>, output: &Output) -> Result<i32> {
    let bin_dir = toolchain_bin_dir()?;
    let manifest_path = installed_manifest_path()?;
    let mut manifest = InstalledManifest::load(&manifest_path)?;

    let mut exit_code = 0;
    let mut changed = false;
    for name in packages {
        // Accept `name@version` for symmetry with `hx install`.
        let name = parse_package_spec(&name).map(|s| s.name).unwrap_or(name);
        let Some(pkg) = manifest.packages.remove(&name) else {
            output.error(&format!("{name} is not installed with `hx install`"));
            output.info("See installed packages with `hx install --list`.");
            exit_code = 1;
            continue;
        };
        for exe in &pkg.executables {
            remove_file_if_present(&bin_dir.join(exe))?;
            output.verbose(&format!("Removed {}", bin_dir.join(exe).display()));
        }
        changed = true;
        output.status(
            "Removed",
            &format!("{name} {} ({})", pkg.version, pkg.executables.join(", ")),
        );
    }

    if changed {
        manifest.save(&manifest_path)?;
    }
    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, version: Option<&str>) -> PackageSpec {
        PackageSpec {
            name: name.to_string(),
            version: version.map(str::to_string),
        }
    }

    #[test]
    fn parses_bare_name() {
        assert_eq!(parse_package_spec("hlint"), Ok(spec("hlint", None)));
        assert_eq!(
            parse_package_spec("optparse-applicative"),
            Ok(spec("optparse-applicative", None))
        );
    }

    #[test]
    fn parses_at_version() {
        assert_eq!(
            parse_package_spec("fourmolu@0.18.0"),
            Ok(spec("fourmolu", Some("0.18.0")))
        );
    }

    #[test]
    fn parses_dash_version() {
        assert_eq!(
            parse_package_spec("hlint-3.8"),
            Ok(spec("hlint", Some("3.8")))
        );
        // A name component with letters is part of the name, not a version.
        assert_eq!(
            parse_package_spec("base64-bytestring"),
            Ok(spec("base64-bytestring", None))
        );
    }

    #[test]
    fn rejects_bad_specs() {
        assert!(parse_package_spec("").is_err());
        assert!(parse_package_spec("hlint@").is_err());
        assert!(parse_package_spec("hlint@latest").is_err());
        assert!(parse_package_spec("foo/bar").is_err());
        assert!(parse_package_spec("123").is_err());
    }

    #[test]
    fn source_requires_something_to_install() {
        assert!(source_from_request(&InstallRequest::default()).is_err());
    }

    #[test]
    fn source_rejects_git_with_path() {
        let req = InstallRequest {
            git: Some("https://x/y.git".into()),
            path: Some(".".into()),
            ..Default::default()
        };
        assert!(source_from_request(&req).is_err());
    }

    #[test]
    fn source_package_selects_within_git_or_path() {
        let req = InstallRequest {
            package: Some("tool".into()),
            path: Some("/src".into()),
            ..Default::default()
        };
        assert_eq!(
            source_from_request(&req),
            Ok(Source::Path {
                path: "/src".into(),
                package: Some("tool".into())
            })
        );
    }

    #[test]
    fn latest_uses_pvp_ordering_not_string_ordering() {
        let versions = vec!["3.9".to_string(), "3.10".to_string(), "3.8.1".to_string()];
        assert_eq!(latest_of(&versions), Some("3.10".to_string()));
        assert_eq!(latest_of(&[]), None);
    }

    #[test]
    fn local_package_picks_single_cabal_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("demo.cabal"),
            "cabal-version: 3.0\nname: demo\nversion: 0.2.1\n",
        )
        .unwrap();
        let got = local_package(dir.path(), None, &Output::new());
        assert_eq!(got, Some(("demo".to_string(), "0.2.1".to_string())));
    }

    #[test]
    fn local_package_finds_named_member_one_level_down() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tool")).unwrap();
        std::fs::write(
            dir.path().join("tool").join("tool.cabal"),
            "cabal-version: 3.0\nname: tool\nversion: 1.0\n",
        )
        .unwrap();
        assert_eq!(local_package(dir.path(), None, &Output::new()), None);
        assert_eq!(
            local_package(dir.path(), Some("tool"), &Output::new()),
            Some(("tool".to_string(), "1.0".to_string()))
        );
    }

    #[test]
    fn staged_executables_are_sorted_file_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b"), "").unwrap();
        std::fs::write(dir.path().join("a"), "").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        assert_eq!(
            staged_executables(dir.path()).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn place_executable_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&dest, "old").unwrap();
        place_executable(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
        assert!(!src.exists());
    }
}
