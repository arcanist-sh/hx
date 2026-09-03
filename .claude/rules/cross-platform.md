# Cross-Platform Guidelines

## Path Handling

### Never Assume Unix Paths
```rust
// Bad
let config_path = format!("{}/.config/hx", home);

// Good
use directories::ProjectDirs;
let dirs = ProjectDirs::from("io", "raskell", "hx").unwrap();
let config_path = dirs.config_dir();
```

### Use `directories` Crate
```rust
use directories::{BaseDirs, ProjectDirs, UserDirs};

// Cache directory
// Linux: ~/.cache/hx
// macOS: ~/Library/Caches/io.raskell.hx
// Windows: C:\Users\<user>\AppData\Local\raskell\hx\cache

// Config directory
// Linux: ~/.config/hx
// macOS: ~/Library/Application Support/io.raskell.hx
// Windows: C:\Users\<user>\AppData\Roaming\raskell\hx\config
```

### Path Separators
```rust
// Bad
let path = format!("{}/subdir/file", dir);

// Good
let path = dir.join("subdir").join("file");
```

## Command Execution

### Never Build Shell Strings
```rust
// Bad
let cmd = format!("ghc {} --make {}", flags, file);
Command::new("sh").arg("-c").arg(cmd);

// Good
Command::new("ghc")
    .args(&flags)
    .arg("--make")
    .arg(file)
    .spawn();
```

### Use Proper Argument Vectors
```rust
pub struct CommandRunner {
    pub fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
}

// Captures stdout/stderr
// Returns structured exit errors
// Supports --verbose passthrough
```

### Executable Extensions
```rust
fn find_executable(name: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };
    which::which(name).ok()
}
```

## Environment Variables

### Home Directory
```rust
// Bad
std::env::var("HOME")

// Good
directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
```

### Path Environment
```rust
// Cross-platform PATH separator
#[cfg(windows)]
const PATH_SEP: char = ';';
#[cfg(not(windows))]
const PATH_SEP: char = ':';
```

## File System

### Line Endings
- Always write with `\n`
- Use `BufReader` which handles both `\n` and `\r\n`
- Don't normalize line endings on read

### File Permissions
```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, Permissions::from_mode(0o755))?;
}
```

### Temp Directories
```rust
// Use tempfile crate
let temp_dir = tempfile::tempdir()?;
// Automatically cleaned up on drop
```

## Platform-Specific Code

Use `cfg` attributes:
```rust
#[cfg(target_os = "linux")]
fn check_native_deps() -> Vec<MissingDep> {
    // Check for libgmp, libz, etc.
}

#[cfg(target_os = "macos")]
fn check_native_deps() -> Vec<MissingDep> {
    // Check via brew or pkg-config
}

#[cfg(target_os = "windows")]
fn check_native_deps() -> Vec<MissingDep> {
    // Windows-specific checks
}
```

## CI Matrix

Test on all platforms:
```yaml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
```

## GHCup Paths

GHCup installs to different locations:
- Linux/macOS: `~/.ghcup/`
- Windows: `C:\ghcup\` or `%APPDATA%\ghcup\`

Always use `ghcup whereis` to find tool locations.
