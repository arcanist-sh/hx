# AGENTS.md

Guidance for AI agents (and the humans configuring them) driving **hx**, the Haskell toolchain CLI.

## What hx is

One binary, `hx`, that replaces the cluster of tools a Haskell project normally needs — `ghcup`, `cabal`, `stack`, `fourmolu`, `hlint`, `hpc` — behind a single, scriptable interface. It manages GHC itself (no `ghcup` required), uses an `hx.toml` manifest and an `hx.lock` lockfile for reproducible builds, and emits structured, actionable errors.

## Fastest way to give an agent tools: the MCP server

```
hx mcp
```

starts a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio (JSON-RPC 2.0). Point an MCP client at it. Example client config:

```json
{
  "mcpServers": {
    "hx": { "command": "hx", "args": ["mcp"] }
  }
}
```

Tools exposed: `hx_build`, `hx_check`, `hx_test`, `hx_run`, `hx_lock`, `hx_sync`, `hx_fmt`, `hx_lint`, `hx_doctor`, `hx_add`, `hx_remove`, `hx_info`, `hx_tree`, `hx_outdated`. Each accepts an optional `cwd` (project directory) and returns the command's combined output plus an `isError` flag. Output is rendered without ANSI colors.

## Driving the CLI directly

- Discover everything: `hx --help`, and `hx <command> --help` for any command.
- Suppress/expand output: `--quiet` (`-q`) and `--verbose` (`-v`) are global. `NO_COLOR` is respected.
- Machine-readable output where it exists: `hx coverage --json`, `hx deps graph --format json`.

### Exit codes — use these to decide success/failure

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Usage error (bad arguments) |
| 3 | Configuration error (bad `hx.toml`) |
| 4 | Toolchain error (GHC/Cabal missing or mismatched) |
| 5 | Build/test failure |
| 6 | Plugin hook failure |

A non-zero exit is meaningful — prefer it over scraping stdout. Errors print a structured summary with a suggested fix on stderr.

### Core commands

| Command | Purpose |
|---------|---------|
| `hx init <name>` | Scaffold a new project |
| `hx build` | Build (`--release`, `--native`, `-j N`) |
| `hx check` | Fast type-check, no binary |
| `hx test` | Run tests (`--pattern <p>`) |
| `hx run [-- args]` | Build and run the executable |
| `hx lock` / `hx sync` | Generate lockfile / build from it |
| `hx fmt [--check]` | Format with fourmolu |
| `hx lint [--fix]` | Run hlint |
| `hx add <pkg> [constraint]` / `hx rm <pkg>` | Manage dependencies |
| `hx info <pkg>` / `hx tree` / `hx outdated` | Inspect dependencies |
| `hx doctor` | Diagnose the toolchain/project (exit 4 if tools missing) |
| `hx toolchain install` | Install/manage GHC |

## Common agent workflows

```bash
# new project, build, run
hx init myapp && cd myapp && hx build && hx run

# add a dependency, rebuild
hx add aeson ">=2.0" && hx build

# reproducible build
hx lock && hx sync

# quality gate before committing
hx fmt --check && hx lint && hx test
```

## Behavior an agent should know

- **Toolchain auto-management.** hx installs and selects GHC itself. The first `hx build` may download a GHC (hundreds of MB). `hx doctor` reports what's present and returns exit 4 when something required is missing.
- **`hx build --native` is experimental** — a fast path for single-package, `base`-only projects. It falls back to cabal otherwise; don't rely on it for multi-dependency projects.
- **Plugins require explicit trust.** Project-local `.hx/plugins/*.scm` scripts do **not** run on a freshly cloned repo. They only run after `hx plugins trust` (recorded in global config, never in the repo). Do not trust untrusted projects' plugins automatically.
- **Manifest + lockfile.** `hx.toml` is the project manifest; `hx.lock` pins toolchain + dependency versions. `hx lock` updates the lockfile; `hx sync` builds against it.

### Minimal `hx.toml`

```toml
[project]
name = "myapp"
kind = "bin"        # or "lib"

[toolchain]
ghc = "9.8.2"
```

## Claude Code skill

A ready-to-use [Agent Skill](https://docs.anthropic.com/en/docs/claude-code/skills) lives at [`.claude/skills/hx/SKILL.md`](.claude/skills/hx/SKILL.md) — copy it into your own `.claude/skills/` (or use this repo's) and Claude will load it automatically when working in an hx project.

## Learn more

- Documentation: https://docs.arcanist.sh/hx/docs/
- Machine-readable project map: https://arcanist.sh/hx/llms.txt
- Source & changelog: https://github.com/arcanist-sh/hx
