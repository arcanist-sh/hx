# asdf-hx

[hx](https://github.com/arcanist-sh/hx) plugin for [asdf](https://asdf-vm.com) and [mise](https://mise.jdx.dev).

## Installation

### mise

```bash
mise plugin install hx https://github.com/raskell-io/asdf-hx.git
mise install hx@latest
mise use hx@latest
```

Or add to your `mise.toml`:

```toml
[tools]
hx = "latest"
```

### asdf

```bash
asdf plugin add hx https://github.com/raskell-io/asdf-hx.git
asdf install hx latest
asdf global hx latest
```

## Usage

```bash
# List all available versions
asdf list all hx

# Install a specific version
asdf install hx 0.4.0

# Set global version
asdf global hx 0.4.0

# Set local version (creates .tool-versions)
asdf local hx 0.4.0
```

## Supported Platforms

- macOS (Apple Silicon and Intel)
- Linux (x86_64 and aarch64)
- Windows (x86_64)

## Links

- [hx documentation](https://github.com/arcanist-sh/hx)
- [asdf documentation](https://asdf-vm.com)
- [mise documentation](https://mise.jdx.dev)

## Binary name

The Helix editor's binary is also called `hx`. asdf and mise create one shim
per executable, so installing as `hx` would put a colliding shim on `PATH`.

When the plugin finds an `hx` that isn't hx, it installs as **`hxs`** instead
and says so. An hx installed elsewhere — or a shim from a previous install of
this plugin — is treated as an upgrade and keeps the name.

Override in either direction:

```bash
HX_BINARY_NAME=hx mise install hx@latest
```
