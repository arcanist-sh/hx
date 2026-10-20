# hx Distribution Channels

Tracking all places where hx is or could be available.

## Live

| Channel | Install Command | Link |
|---------|-----------------|------|
| GitHub Releases | `curl -fsSL https://arcanist.sh/hx/install.sh \| sh` | [releases](https://github.com/arcanist-sh/hx/releases) |
| Homebrew | `brew install arcanist-sh/tap/hx` | [homebrew-tap](https://github.com/arcanist-sh/homebrew-tap) |
| Nix Flake | `nix run github:arcanist-sh/hx` | [flake.nix](../flake.nix) |
| Scoop | `scoop bucket add raskell-io ...` | [scoop-bucket](https://github.com/raskell-io/scoop-bucket) |
| aqua | `aqua g -i arcanist-sh/hx` | [aqua-registry](https://github.com/aquaproj/aqua-registry) |
| pkgx | `pkgx hx` | [pantry](https://github.com/pkgxdev/pantry) |
| WinGet | `winget install raskell-io.hx` | [winget-pkgs](https://github.com/microsoft/winget-pkgs/tree/master/manifests/r/raskell-io/hx); each release is submitted by [winget.yml](../.github/workflows/winget.yml) (needs the `WINGET_TOKEN` secret) |
| crates.io | `cargo install hx-cli` | [hx-cli](https://crates.io/crates/hx-cli) (all crates published by [release.yml](../.github/workflows/release.yml)) |
| FlakeHub | `nix profile install "https://flakehub.com/f/arcanist-sh/hx/*"` | [workflow](../.github/workflows/flakehub.yml) |
| Docker/GHCR | `docker run --rm ghcr.io/arcanist-sh/hx --help` | [workflow](../.github/workflows/docker.yml) |

## Pending (PRs submitted)

| Channel | Status | Link |
|---------|--------|------|
| Nixpkgs | PR open (maintainers want more adoption first) | https://github.com/NixOS/nixpkgs/pull/483671 |
| asdf | PR open | https://github.com/asdf-vm/asdf-plugins/pull/1140 |
| mise | Uses asdf plugin | (waiting on asdf PR) |

## Pending (manual action needed)

| Channel | Blocker | Files Ready |
|---------|---------|-------------|
| AUR | Need Arch Linux to solve CAPTCHA for account creation | [aur-hx-bin](https://github.com/raskell-io/aur-hx-bin) |
| Chocolatey | Need Windows + Chocolatey account to publish | [chocolatey-hx](https://github.com/raskell-io/chocolatey-hx) |

## Plugin/Package repos

| Repo | Purpose |
|------|---------|
| [raskell-io/asdf-hx](https://github.com/raskell-io/asdf-hx) | asdf plugin |
| [raskell-io/mise-hx](https://github.com/raskell-io/mise-hx) | mise plugin |
| [arcanist-sh/homebrew-tap](https://github.com/arcanist-sh/homebrew-tap) | Homebrew formula |
| [raskell-io/scoop-bucket](https://github.com/raskell-io/scoop-bucket) | Scoop bucket |
| [raskell-io/aur-hx-bin](https://github.com/raskell-io/aur-hx-bin) | AUR PKGBUILD |
| [raskell-io/chocolatey-hx](https://github.com/raskell-io/chocolatey-hx) | Chocolatey package |
