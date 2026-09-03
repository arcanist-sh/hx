//! Binary distribution command.
//!
//! Creates distributable release archives with binaries, completions, and metadata.

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use clap_complete::{Shell, generate};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::Cli;
use hx_ui::Output;

/// Distribution configuration.
pub struct DistConfig {
    pub target: Option<String>,
    pub output_dir: PathBuf,
    pub strip: bool,
    pub include_completions: bool,
    pub version: Option<String>,
}

/// Get the current platform's target triple.
fn current_target() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    match (arch, os) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("x86_64", "windows") => "x86_64-pc-windows-msvc".to_string(),
        _ => format!("{}-{}", arch, os),
    }
}

/// Get version from Cargo.toml or CLI override.
fn get_version(override_version: Option<String>) -> Result<String> {
    if let Some(v) = override_version {
        return Ok(v);
    }

    // Try to get version from Cargo.toml
    let cargo_toml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("Cargo.toml"))
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));

    if cargo_toml.exists() {
        let content = fs::read_to_string(&cargo_toml).context("Failed to read Cargo.toml")?;

        // Simple TOML parsing for version
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("version") && line.contains('=') {
                if let Some(version) = line.split('=').nth(1) {
                    let version = version.trim().trim_matches('"').trim_matches('\'');
                    return Ok(version.to_string());
                }
            }
        }
    }

    // Fallback to compiled version
    Ok(env!("CARGO_PKG_VERSION").to_string())
}

/// Build release binary for the specified target.
fn build_release(target: &str, output: &Output) -> Result<PathBuf> {
    output.status("Building", &format!("release binary for {}", target));

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "--target", target, "-p", "hx-cli"]);

    let status = cmd.status().context("Failed to spawn cargo build")?;

    if !status.success() {
        bail!("Cargo build failed with exit code: {:?}", status.code());
    }

    // Find the built binary
    let binary_name = if target.contains("windows") {
        "hx.exe"
    } else {
        "hx"
    };
    let binary_path = PathBuf::from("target")
        .join(target)
        .join("release")
        .join(binary_name);

    if !binary_path.exists() {
        bail!("Built binary not found at: {}", binary_path.display());
    }

    Ok(binary_path)
}

/// Strip debug symbols from the binary.
fn strip_binary(binary_path: &Path, target: &str, output: &Output) -> Result<()> {
    output.verbose("Stripping debug symbols...");

    // Windows doesn't need stripping, and cross-compiled binaries need target-specific strip
    if target.contains("windows") {
        return Ok(());
    }

    let strip_cmd =
        if target.contains("linux") && !target.contains("musl") || target.contains("darwin") {
            "strip"
        } else {
            // For cross-compilation, try target-prefixed strip
            return Ok(()); // Skip if not available
        };

    let status = Command::new(strip_cmd).arg(binary_path).status();

    match status {
        Ok(s) if s.success() => {
            output.verbose("Binary stripped successfully");
            Ok(())
        }
        Ok(_) => {
            output.warn("Strip command failed, continuing without stripping");
            Ok(())
        }
        Err(_) => {
            output.verbose("Strip not available, skipping");
            Ok(())
        }
    }
}

/// Create staging directory structure.
fn create_staging(config: &DistConfig, target: &str, version: &str) -> Result<PathBuf> {
    let archive_name = format!("hx-v{}-{}", version, target);
    let staging_dir = config.output_dir.join(&archive_name);

    // Clean and create staging directory
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).context("Failed to remove existing staging directory")?;
    }
    fs::create_dir_all(&staging_dir).context("Failed to create staging directory")?;

    // Create completions subdirectory
    if config.include_completions {
        fs::create_dir_all(staging_dir.join("completions"))
            .context("Failed to create completions directory")?;
    }

    Ok(staging_dir)
}

/// Copy binary to staging directory.
fn copy_binary(staging_dir: &Path, binary_path: &Path, target: &str) -> Result<()> {
    let binary_name = if target.contains("windows") {
        "hx.exe"
    } else {
        "hx"
    };
    let dest = staging_dir.join(binary_name);

    fs::copy(binary_path, &dest).context("Failed to copy binary to staging")?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

/// Copy metadata files to staging directory.
fn copy_metadata(staging_dir: &Path, output: &Output) -> Result<()> {
    // Find project root (where README.md and LICENSE should be)
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let files = ["README.md", "LICENSE", "CHANGELOG.md"];

    for file in &files {
        let src = project_root.join(file);
        if src.exists() {
            let dest = staging_dir.join(file);
            fs::copy(&src, &dest).with_context(|| format!("Failed to copy {}", file))?;
            output.verbose(&format!("Copied {}", file));
        }
    }

    Ok(())
}

/// Generate shell completions to staging directory.
fn generate_completions(staging_dir: &Path, output: &Output) -> Result<()> {
    let completions_dir = staging_dir.join("completions");

    let shells = [
        (Shell::Bash, "hx.bash"),
        (Shell::Fish, "hx.fish"),
        (Shell::Zsh, "hx.zsh"),
        (Shell::PowerShell, "_hx.ps1"),
    ];

    for (shell, filename) in &shells {
        let path = completions_dir.join(filename);
        let mut file =
            File::create(&path).with_context(|| format!("Failed to create {}", filename))?;

        // Deliberately the default name, not the invoked one: these ship inside
        // the release archive, before the installer decides what to call the
        // binary on this machine. An alternate install regenerates them with
        // `completions install`.
        let mut cmd = Cli::command();
        generate(*shell, &mut cmd, crate::cli::DEFAULT_NAME, &mut file);

        output.verbose(&format!("Generated {} completions", filename));
    }

    Ok(())
}

/// Create archive from staging directory.
fn create_archive(
    staging_dir: &Path,
    config: &DistConfig,
    target: &str,
    _version: &str,
    output: &Output,
) -> Result<PathBuf> {
    let archive_name = staging_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid staging directory name")?;

    let archive_path = if target.contains("windows") {
        let path = config.output_dir.join(format!("{}.zip", archive_name));
        create_zip(staging_dir, &path, output)?;
        path
    } else {
        let path = config.output_dir.join(format!("{}.tar.gz", archive_name));
        create_tarball(staging_dir, &path, output)?;
        path
    };

    Ok(archive_path)
}

/// Create a tar.gz archive.
fn create_tarball(staging_dir: &Path, archive_path: &Path, output: &Output) -> Result<()> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    output.verbose(&format!("Creating tarball: {}", archive_path.display()));

    let tar_gz = File::create(archive_path).context("Failed to create tarball file")?;
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);

    // Get the directory name for the archive root
    let _archive_name = staging_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid staging directory name")?;

    // Add all files from staging directory
    for entry in walkdir::WalkDir::new(staging_dir) {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();

        if path == staging_dir {
            continue;
        }

        let relative_path = path
            .strip_prefix(staging_dir.parent().unwrap_or(staging_dir))
            .context("Failed to get relative path")?;

        if path.is_file() {
            tar.append_path_with_name(path, relative_path)
                .with_context(|| format!("Failed to add {} to tarball", path.display()))?;
        } else if path.is_dir() {
            tar.append_dir(relative_path, path).with_context(|| {
                format!("Failed to add directory {} to tarball", path.display())
            })?;
        }
    }

    tar.finish().context("Failed to finish tarball")?;

    Ok(())
}

/// Create a zip archive.
fn create_zip(staging_dir: &Path, archive_path: &Path, output: &Output) -> Result<()> {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    output.verbose(&format!("Creating zip: {}", archive_path.display()));

    let file = File::create(archive_path).context("Failed to create zip file")?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir::WalkDir::new(staging_dir) {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();

        if path == staging_dir {
            continue;
        }

        let relative_path = path
            .strip_prefix(staging_dir.parent().unwrap_or(staging_dir))
            .context("Failed to get relative path")?;
        let name = relative_path.to_string_lossy();

        if path.is_file() {
            zip.start_file(name.as_ref(), options)
                .with_context(|| format!("Failed to start file {} in zip", name))?;
            let mut f = File::open(path)?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        } else if path.is_dir() && path != staging_dir {
            zip.add_directory(name.as_ref(), options)
                .with_context(|| format!("Failed to add directory {} to zip", name))?;
        }
    }

    zip.finish().context("Failed to finish zip")?;

    Ok(())
}

/// Generate SHA256 checksum file.
fn generate_checksum(archive_path: &Path, output: &Output) -> Result<PathBuf> {
    output.verbose("Generating SHA256 checksum...");

    let mut file = File::open(archive_path).context("Failed to open archive for checksum")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let hash = hasher.finalize();
    let hash_hex = hex::encode(hash);

    let archive_name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid archive filename")?;

    // Use simpler extension handling
    let checksum_path = PathBuf::from(format!("{}.sha256", archive_path.display()));

    let checksum_content = format!("{}  {}\n", hash_hex, archive_name);
    fs::write(&checksum_path, &checksum_content).context("Failed to write checksum file")?;

    Ok(checksum_path)
}

/// Run the dist command - build and package a release.
pub async fn run(config: DistConfig, output: &Output) -> Result<i32> {
    // Ensure output directory exists
    fs::create_dir_all(&config.output_dir).context("Failed to create output directory")?;

    // Get version
    let version = get_version(config.version.clone())?;
    output.status("Version", &version);

    // Determine target
    let target = config.target.clone().unwrap_or_else(current_target);
    output.status("Target", &target);

    // Build release binary
    let binary_path = build_release(&target, output)?;
    output.status("Built", "release binary");

    // Strip if requested
    if config.strip {
        strip_binary(&binary_path, &target, output)?;
    }

    // Create staging directory
    let staging_dir = create_staging(&config, &target, &version)?;
    output.verbose(&format!("Staging: {}", staging_dir.display()));

    // Copy binary
    copy_binary(&staging_dir, &binary_path, &target)?;
    output.verbose("Binary copied");

    // Copy metadata
    copy_metadata(&staging_dir, output)?;

    // Generate completions
    if config.include_completions {
        generate_completions(&staging_dir, output)?;
        output.verbose("Completions generated");
    }

    // Create archive
    let archive_path = create_archive(&staging_dir, &config, &target, &version, output)?;
    output.status("Archive", &archive_path.display().to_string());

    // Generate checksum
    let checksum_path = generate_checksum(&archive_path, output)?;
    output.status("Checksum", &checksum_path.display().to_string());

    // Clean up staging directory
    fs::remove_dir_all(&staging_dir).context("Failed to clean up staging directory")?;

    output.status("Created", &archive_path.display().to_string());

    Ok(0)
}

/// Generate Homebrew formula.
pub fn generate_formula(
    version: Option<String>,
    output_file: Option<PathBuf>,
    output: &Output,
) -> Result<i32> {
    let version = get_version(version)?;

    // Use a constant for the Ruby interpolation to avoid Rust lexer issues
    let bin_interpolation = r#"#{bin}"#;

    let formula = format!(
        r#"class Hx < Formula
  desc "Fast, opinionated Haskell toolchain CLI"
  homepage "https://arcanist.sh/hx/"
  version "{version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/arcanist-sh/hx/releases/download/v{version}/hx-v{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_ARM64_DARWIN"
    else
      url "https://github.com/arcanist-sh/hx/releases/download/v{version}/hx-v{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X64_DARWIN"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/arcanist-sh/hx/releases/download/v{version}/hx-v{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_ARM64_LINUX"
    else
      url "https://github.com/arcanist-sh/hx/releases/download/v{version}/hx-v{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X64_LINUX"
    end
  end

  def install
    # Installed as `hxs`, not `hx`: the Helix editor already provides an `hx`
    # binary and ships in homebrew-core, so `bin.install "hx"` fails at the
    # link step for anyone who has it. Homebrew formulae have no install-time
    # choice, so unlike install.sh we cannot detect and adapt -- we have to
    # pick one name for everyone.
    #
    # Rename rather than `conflicts_with "helix"`. Helix's own conflicts
    # (evil-helix, hex) are all editors, genuine alternatives to it; a Haskell
    # toolchain is not, and someone may reasonably want both. This follows the
    # same convention as fcrackzip renaming zipinfo to fcrackzipinfo.
    bin.install "hx" => "hxs"

    # Generate completions from the installed binary rather than shipping the
    # archive's copies: those are generated under the default name and would
    # register against `hx`, a command this install does not provide.
    (bash_completion/"hxs").write Utils.safe_popen_read("{bin_interpolation}/hxs", "completions", "generate", "bash")
    (fish_completion/"hxs.fish").write Utils.safe_popen_read("{bin_interpolation}/hxs", "completions", "generate", "fish")
    (zsh_completion/"_hxs").write Utils.safe_popen_read("{bin_interpolation}/hxs", "completions", "generate", "zsh")
  end

  def caveats
    <<~EOS
      Installed as `hxs` rather than `hx`, because the Helix editor already
      provides an `hx` binary. Every command works the same, spelled `hxs`.

      If you do not use Helix and want the shorter name:
        ln -s {bin_interpolation}/hxs $HOMEBREW_PREFIX/bin/hx
    EOS
  end

  test do
    # The version banner is named from argv[0] as of #19, so a binary
    # installed as hxs reports "hxs <version>", not "hx <version>".
    assert_match "hxs #{{version}}", shell_output("{bin_interpolation}/hxs --version")
  end
end
"#,
        version = version,
        bin_interpolation = bin_interpolation
    );

    match output_file {
        Some(path) => {
            fs::write(&path, &formula)
                .with_context(|| format!("Failed to write formula to {}", path.display()))?;
            output.status("Written", &path.display().to_string());
        }
        None => {
            println!("{}", formula);
        }
    }

    output.info("Note: Replace SHA256 placeholders with actual checksums from release artifacts");

    Ok(0)
}

/// Generate installation script.
pub fn generate_install_script(
    version: Option<String>,
    output_file: Option<PathBuf>,
    output: &Output,
) -> Result<i32> {
    let version = get_version(version)?;

    let script = format!(
        r#"#!/bin/sh
# hx installer script
# Usage: curl -fsSL https://get.hx.dev | sh
#    or: sh install.sh

set -e

VERSION="{version}"
REPO="arcanist-sh/hx"
INSTALL_DIR="${{HX_INSTALL_DIR:-/usr/local/bin}}"

# Colors (if terminal supports it)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    NC=''
fi

info() {{
    printf "${{GREEN}}info${{NC}}: %s\n" "$1"
}}

warn() {{
    printf "${{YELLOW}}warn${{NC}}: %s\n" "$1"
}}

error() {{
    printf "${{RED}}error${{NC}}: %s\n" "$1" >&2
    exit 1
}}

# Detect platform
detect_platform() {{
    OS=$(uname -s)
    ARCH=$(uname -m)

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64) echo "aarch64-apple-darwin" ;;
                x86_64) echo "x86_64-apple-darwin" ;;
                *) echo "unsupported" ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                aarch64) echo "aarch64-unknown-linux-gnu" ;;
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                *) echo "unsupported" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$ARCH" in
                x86_64) echo "x86_64-pc-windows-msvc" ;;
                *) echo "unsupported" ;;
            esac
            ;;
        *) echo "unsupported" ;;
    esac
}}

# Check for required commands
check_cmd() {{
    command -v "$1" >/dev/null 2>&1
}}

need_cmd() {{
    if ! check_cmd "$1"; then
        error "Required command '$1' not found"
    fi
}}

# Download file
download() {{
    url="$1"
    dest="$2"

    if check_cmd curl; then
        curl -fsSL "$url" -o "$dest"
    elif check_cmd wget; then
        wget -q "$url" -O "$dest"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}}

main() {{
    info "Installing hx v$VERSION"

    TARGET=$(detect_platform)
    if [ "$TARGET" = "unsupported" ]; then
        error "Unsupported platform: $(uname -s) $(uname -m)"
    fi

    info "Detected platform: $TARGET"

    # Determine archive extension
    case "$TARGET" in
        *windows*) EXT="zip" ;;
        *) EXT="tar.gz" ;;
    esac

    ARCHIVE="hx-v${{VERSION}}-${{TARGET}}.${{EXT}}"
    URL="https://github.com/${{REPO}}/releases/download/v${{VERSION}}/${{ARCHIVE}}"
    CHECKSUM_URL="${{URL}}.sha256"

    # Create temp directory
    TMPDIR=$(mktemp -d)
    trap "rm -rf $TMPDIR" EXIT

    info "Downloading $ARCHIVE..."
    download "$URL" "$TMPDIR/$ARCHIVE"

    # Verify checksum if sha256sum is available
    if check_cmd sha256sum; then
        info "Verifying checksum..."
        download "$CHECKSUM_URL" "$TMPDIR/$ARCHIVE.sha256"
        (cd "$TMPDIR" && sha256sum -c "$ARCHIVE.sha256")
    elif check_cmd shasum; then
        info "Verifying checksum..."
        download "$CHECKSUM_URL" "$TMPDIR/$ARCHIVE.sha256"
        EXPECTED=$(cut -d' ' -f1 "$TMPDIR/$ARCHIVE.sha256")
        ACTUAL=$(shasum -a 256 "$TMPDIR/$ARCHIVE" | cut -d' ' -f1)
        if [ "$EXPECTED" != "$ACTUAL" ]; then
            error "Checksum verification failed"
        fi
    else
        warn "sha256sum not found, skipping checksum verification"
    fi

    info "Extracting..."
    case "$EXT" in
        "tar.gz")
            tar -xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"
            ;;
        "zip")
            need_cmd unzip
            unzip -q "$TMPDIR/$ARCHIVE" -d "$TMPDIR"
            ;;
    esac

    # Find the binary
    BINARY=$(find "$TMPDIR" -name "hx" -o -name "hx.exe" | head -n1)
    if [ -z "$BINARY" ]; then
        error "Binary not found in archive"
    fi

    # Install binary
    info "Installing to $INSTALL_DIR..."
    if [ -w "$INSTALL_DIR" ]; then
        cp "$BINARY" "$INSTALL_DIR/"
        chmod +x "$INSTALL_DIR/hx"
    else
        sudo mkdir -p "$INSTALL_DIR"
        sudo cp "$BINARY" "$INSTALL_DIR/"
        sudo chmod +x "$INSTALL_DIR/hx"
    fi

    # Verify installation
    if check_cmd "$INSTALL_DIR/hx"; then
        info "hx v$VERSION installed successfully!"
        info "Run 'hx --help' to get started"
    else
        warn "Installation complete, but hx not found in PATH"
        warn "Add $INSTALL_DIR to your PATH"
    fi

    # Completions hint
    info ""
    info "To install shell completions, run:"
    info "  hx completions bash > ~/.local/share/bash-completion/completions/hx"
    info "  hx completions zsh > ~/.zfunc/_hx"
    info "  hx completions fish > ~/.config/fish/completions/hx.fish"
}}

main "$@"
"#,
        version = version
    );

    match output_file {
        Some(path) => {
            fs::write(&path, &script)
                .with_context(|| format!("Failed to write script to {}", path.display()))?;

            // Make executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }

            output.status("Written", &path.display().to_string());
        }
        None => {
            println!("{}", script);
        }
    }

    Ok(0)
}
