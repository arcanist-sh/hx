# Lockfile Format

The user-facing reference for `hx.lock` (every field, versioning, the offline
workflow and `hx lock --check`) is [docs/LOCKFILE.md](../../../docs/LOCKFILE.md).
This page covers the Rust API.

## Types

```rust
pub const LOCK_VERSION: u32 = 2;     // format `hx lock` writes
pub const MIN_LOCK_VERSION: u32 = 1; // oldest format still read

pub struct Lockfile {
    pub version: u32,                // as read from the file (see needs_upgrade)
    pub created_at: DateTime<Utc>,
    pub toolchain: LockedToolchain,  // ghc, cabal, bhc, bhc_profile
    pub plan: LockedPlan,            // compiler_id, platform, index_state,
                                     // snapshot, bhc_platform, hash
    pub workspace: LockedWorkspace,
    pub packages: Vec<LockedPackage>,
}

pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: String,              // hackage | stackage | ghc | git | path
    pub sha256: Option<String>,      // sdist hash (v1 spelling `hash` accepted)
    pub revision: Option<u32>,       // Hackage .cabal revision
    pub cabal_sha256: Option<String>,
    pub flags: BTreeMap<String, bool>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub subdir: Option<String>,
    pub path: Option<String>,
}
```

## Reading and writing

```rust
use hx_lock::{LockedPackage, Lockfile};

let mut lock = Lockfile::from_file("hx.lock")?; // v1 and v2 both load
if lock.needs_upgrade() {
    // read from an older format; writing it back upgrades it
}

let mut pkg = LockedPackage::hackage("text", "2.1.1");
pkg.sha256 = Some("…".into());
lock.add_package(pkg);

lock.to_file("hx.lock")?; // always writes v2, packages sorted by name
```

`Lockfile::parse` rejects a format newer than `LOCK_VERSION` with
`LockError::UnsupportedVersion` instead of misreading it.

## Helpers

| Function | Purpose |
|----------|---------|
| `Lockfile::same_content(&other)` | Equal pins, ignoring `created_at` and the format version read |
| `Lockfile::fetchable_sdists()` | Hackage/Stackage packages a build may download (excludes compiler-internal ones such as `base`) |
| `Lockfile::cabal_args()` | `--index-state` and `--constraint` arguments that pin `cabal build` to the lockfile |
| `Lockfile::fingerprint()` | Plan hash; absent v2 fields contribute nothing, so v1 data keeps its fingerprint |
| `is_non_reinstallable(name)` | Whether a package is tied to the compiler and never built from an sdist |
| `normalize_sha256(s)` | Strip a `sha256:` prefix and lowercase |

## Determinism

`to_string()` writes the current version, sorts packages by name (then
version) and workspace members by name, and sorts flags by name, so
re-serializing a parsed lockfile is byte-identical. A golden snapshot test
(`src/snapshots/hx_lock__tests__lockfile_v2.snap`) guards the layout.
