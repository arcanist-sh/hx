# Release Metadata

hx learns which GHC, Cabal and HLS releases exist, which one is recommended,
and where each bindist lives from the metadata file ghcup publishes:

```
https://raw.githubusercontent.com/haskell/ghcup-metadata/master/ghcup-0.1.0.yaml
```

This replaces hand-kept version and URL tables, so new releases, renamed
assets and per-distribution builds reach hx without a new hx release. The
built-in tables (`KNOWN_GHC_VERSIONS`, `RECOMMENDED_GHC_VERSION`, the
`bindist_suffix` / `cabal_platform_suffix` URL scheme) remain as the offline
fallback.

## What hx reads

Only `ghcupDownloads.{GHC,Cabal,HLS}.toolVersions` is parsed. For each
version hx keeps:

| Field | Used for |
|-------|----------|
| `viTags` | `Recommended`, `Latest`, `old`, prerelease tags |
| `viArch.<arch>.<os>.<range>.dlUri` | Download URL |
| `...dlSubdir` | Directory the GHC bindist unpacks into |
| `...dlHash` | Expected SHA-256 of the archive |
| `...dlOutput` | File name to save the archive under |

YAML anchors and aliases are resolved while loading. Install specs, tool
requirements, post-install notes and the other tools (GHCup, Stack) are
ignored.

## Fetching and caching

| Situation | Result |
|-----------|--------|
| Cache younger than 24 hours | Used, no network |
| Cache older, refresh succeeds | Refreshed copy used and cached |
| Refresh fails | Stale cache used |
| No cache, refresh fails | Built-in tables |
| `HX_OFFLINE=1` | Cache of any age, else built-in tables; never fetches |

The cache lives in hx's global cache directory (`$XDG_CACHE_HOME/hx` when
set, otherwise `~/.cache/hx` on Linux and `~/Library/Caches/io.raskell.hx`
on macOS) under `ghcup/ghcup-metadata-<hash of URL>.yaml`. A download is
only cached once it parses, so an error page never replaces a good copy.
Refreshes time out after 10 seconds.

`HX_GHCUP_METADATA_URL` points hx at another copy (a mirror, or a
`file://` path for testing). Each URL has its own cache file.

Metadata problems never fail a command: every caller falls back to the
built-in tables.

## Bindist selection

For installs on the machine hx runs on, hx describes the host the way ghcup
does:

- **Architecture**: `A_64` (x86_64) or `A_ARM64` (aarch64)
- **OS**: `Darwin`, `Windows`, or `Linux_<Distro>`
- **Distribution**: from `/etc/os-release` (`ID`, `VERSION_ID`, `ID_LIKE`)

The distribution `ID` maps to ghcup's names (`debian` → `Linux_Debian`,
`ubuntu` → `Linux_Ubuntu`, `linuxmint` → `Linux_Mint`, `fedora`, `centos`,
`rhel` → `Linux_RedHat`, `alpine`, `amzn` → `Linux_AmazonLinux`, `rocky`,
`void`, `gentoo`, `exherbo`, `opensuse*`/`sles` → `Linux_OpenSUSE`).

A bindist is then chosen in this order:

1. The distribution's build whose version range (e.g. `( >= 10 && < 11 )`,
   `>= 12`) contains `VERSION_ID`
2. The distribution's `unknown_versioning` build
3. The `Linux_UnknownLinux` build

Examples with GHC 9.10.3 on x86_64:

| Host | Bindist |
|------|---------|
| Debian 12 | `deb12` |
| Ubuntu 24.04 | `ubuntu20_04` (Ubuntu's unversioned build) |
| Fedora 40 | `fedora38` |
| Alpine 3.20 | `alpine3_18` |
| Arch, or an unknown distribution | `rocky8` (`Linux_UnknownLinux`) |

hx goes slightly beyond ghcup in two places:

- If the `ID` is unknown, each `ID_LIKE` entry is tried, without a version
  (Pop!_OS with `ID_LIKE="ubuntu debian"` gets Ubuntu's unversioned build;
  its own version number says nothing about Ubuntu's).
- A system whose C library is musl (a `/lib/ld-musl-*` loader and no glibc
  loader) is treated as Alpine, so musl systems get musl builds.

When metadata has no entry for the version or host, or the entry is an
archive format hx cannot unpack (only `.tar.xz`, and `.zip` for Cabal on
Windows), the built-in URL is used. Cabal 3.18.2.0, for example, is not in
ghcup's metadata yet and installs from the built-in URL.

## Checksums

A metadata-provided `dlHash` is the expected SHA-256; the download is
verified against it, including when a previously downloaded archive is
reused. Without metadata, hx falls back to the `SHA256SUMS` manifest next to
the archive, as before.

## Where it is used

| Command | Use |
|---------|-----|
| `hx toolchain install` | URL, unpack directory and SHA-256 for GHC and Cabal |
| `hx toolchain list` | Versions, `Recommended`/`Latest` markers |
| `hx doctor` | Mentions (as info) a newer recommended GHC or Cabal than the one on PATH, unless `hx.toml` pins it |

Versions pinned in `hx.toml` always win; the recommended version is only a
suggestion. `hx init`, `hx import` and the Nix integration keep using the
built-in `RECOMMENDED_GHC_VERSION`, so the files they generate do not depend
on the network.

HLS is installed through ghcup; its releases are parsed and available
through the API, but nothing in hx installs from them yet. The metadata does not say which GHC versions each HLS release
supports, so that table stays in `hx-config`.

## API

```rust
use hx_toolchain::metadata::{self, Host, Tool};
use hx_toolchain::Platform;

// Cached / refreshed per the rules above; None means "use built-in tables".
let meta = metadata::load().await;

let recommended = metadata::recommended_version(meta.as_deref(), Tool::Ghc);

let platform = Platform::current().unwrap();
let host = Host::detect(platform);
if let Some(m) = meta.as_deref()
    && let Some(dl) = m.download(Tool::Ghc, "9.10.3", &host)
{
    println!("{} sha256={:?}", dl.url, dl.sha256);
}
```

## What hx does not mirror

- **Other channels**: only `ghcup-0.1.0.yaml` is read; ghcup's prerelease,
  cross and vanilla channels are not merged in. Use `HX_GHCUP_METADATA_URL`
  to point at a different file.
- **Install specs** (`dlInstallSpec`): hx keeps its own install steps
  (`./configure && make install` for GHC, a single binary for Cabal).
- **Regex unpack directories** (`dlSubdir: {RegexDir: ...}`): ignored; the
  usual directory names are searched instead.
- **macOS and Windows version ranges**: only the `unknown_versioning` entry
  is used on those platforms.
- **FreeBSD and 32-bit builds**: hx has no direct-install support for them.
- **Distribution package requirements** (`toolRequirements`): not shown;
  `hx doctor` keeps its own native dependency checks.
