# Freeze File Integration

hx can import package versions from Cabal's freeze files.

## Overview

Cabal freeze files (`cabal.project.freeze`) pin exact package versions. hx can parse these to populate the lockfile.

## Freeze File Format

```
-- cabal.project.freeze
constraints: any.aeson ==2.2.1.0,
             any.base ==4.19.0.0,
             any.bytestring ==0.12.0.2,
             any.text ==2.1,
             any.vector ==0.13.1.0
```

## Parsing Freeze Files

```rust
use hx_lock::{parse_freeze_file, parse_freeze_index_state};

let content = std::fs::read_to_string("cabal.project.freeze")?;
for pkg in parse_freeze_file(&content) {
    println!("{} == {} {:?}", pkg.name, pkg.version, pkg.flag_string());
}
let index_state = parse_freeze_index_state(&content); // e.g. Some("2024-01-15T00:00:00Z")
```

### Parsing Rules

1. Lines starting with `--` are comments
2. `constraints:` starts the constraint list; continuation lines follow
3. Version constraints are `any.{name} =={version}` (modern Cabal) or
   `{name} =={version}`
4. Flag constraints (`aeson -cffi +ordered-keymap`) are attached to their
   package as `flags`
5. `index-state:` is read by `parse_freeze_index_state`, in both the plain and
   the per-repository (`hackage.haskell.org <timestamp>`) form
6. Source constraints and other stanzas are ignored

## CLI Usage

`hx lock --cabal` runs `cabal freeze`, converts the freeze file with
`parse_freeze_file`, and then pins source hashes and `.cabal` revisions from
Cabal's Hackage index (as of the freeze file's `index-state`, if it has one).
