---
name: hx
description: Build, test, run, lint, format, and manage dependencies for Haskell projects using the hx toolchain CLI. Use whenever working in a Haskell project that has an hx.toml (or when setting one up), or when the user asks to build/test/run/typecheck Haskell code, manage GHC/Cabal versions, or add/remove Hackage dependencies.
---

# hx — Haskell toolchain

`hx` is a single CLI that replaces ghcup, cabal, stack, fourmolu, hlint, and hpc. It manages GHC itself, uses an `hx.toml` manifest and an `hx.lock` lockfile, and returns meaningful exit codes.

## Detecting that this skill applies

The project is an hx project if there is an `hx.toml` at (or above) the working directory. If the user has a `.cabal`/`stack.yaml` but no `hx.toml`, you can adopt it with `hx init` or `hx import --from stack`.

## Core commands

| Goal | Command |
|------|---------|
| Scaffold a project | `hx init <name>` |
| Build | `hx build` (`--release`, `-j N`) |
| Type-check only (fast) | `hx check` |
| Run tests | `hx test` (`--pattern <p>`) |
| Build & run | `hx run -- <args>` |
| Format / check format | `hx fmt` / `hx fmt --check` |
| Lint / autofix | `hx lint` / `hx lint --fix` |
| Add / remove a dependency | `hx add <pkg> "<constraint>"` / `hx rm <pkg>` |
| Lock / build from lock | `hx lock` / `hx sync` |
| Diagnose the environment | `hx doctor` |
| Install/manage GHC | `hx toolchain install` |

Run `hx <command> --help` for flags. `--quiet`/`--verbose` are global; `NO_COLOR` is respected.

## Interpreting results — exit codes

Decide success/failure from the exit code, not by scraping output:

- `0` success · `1` general error · `2` bad arguments · `3` bad `hx.toml`
- `4` toolchain error (GHC/Cabal missing or mismatched — run `hx doctor`)
- `5` build/test failure · `6` plugin hook failure

Errors print a structured summary with a suggested fix on stderr; surface that fix to the user.

## Typical workflows

```bash
# new project
hx init myapp && cd myapp && hx build && hx run

# add a dependency and rebuild
hx add aeson ">=2.0" && hx build

# pre-commit quality gate
hx fmt --check && hx lint && hx test

# reproducible build
hx lock && hx sync
```

## Important behavior

- **Toolchain auto-install:** the first `hx build` may download a GHC (hundreds of MB). `hx doctor` returns exit `4` when a required tool is missing and tells you how to install it.
- **`hx build --native` is experimental** — a fast path for single-package, `base`-only projects; it falls back to cabal otherwise. Don't use it for multi-dependency projects.
- **Plugins require trust:** project-local `.hx/plugins/*.scm` scripts do **not** run on a freshly cloned repo. Never run `hx plugins trust` on a project you don't trust.

## Programmatic / MCP use

For tool-call access instead of shelling out, run `hx mcp` (an MCP server over stdio) — it exposes build/check/test/run/lock/sync/fmt/lint/doctor and dependency tools. See `AGENTS.md` in the repo.
