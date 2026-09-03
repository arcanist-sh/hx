#!/bin/sh
# hx installer script
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arcanist-sh/hx/main/install.sh | sh
#
# Options (via environment variables):
#   HX_VERSION      - Specific version to install (default: latest)
#   HX_INSTALL_DIR  - Installation directory (default: ~/.local/bin or /usr/local/bin)
#   HX_NO_MODIFY_PATH - Set to skip PATH modification suggestions
#   HX_ALLOW_UNVERIFIED - Set to 1 to proceed when checksum verification is impossible
#   HX_BINARY_NAME  - Name to install the binary as (default: hx, or hxs when
#                     another program already owns `hx` on PATH)
#
# Options (via flags, e.g. `curl -fsSL ... | sh -s -- --binary-name hxs`):
#   --binary-name <name>  - Same as HX_BINARY_NAME

set -e

REPO="arcanist-sh/hx"
GITHUB_API="https://api.github.com/repos/${REPO}/releases/latest"

# Colors (if terminal supports it)
if [ -t 1 ]; then
    BOLD='\033[1m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    RED='\033[0;31m'
    CYAN='\033[0;36m'
    NC='\033[0m'
else
    BOLD=''
    GREEN=''
    YELLOW=''
    RED=''
    CYAN=''
    NC=''
fi

info() {
    printf "${GREEN}info${NC}: %s\n" "$1"
}

warn() {
    printf "${YELLOW}warn${NC}: %s\n" "$1"
}

error() {
    printf "${RED}error${NC}: %s\n" "$1" >&2
    exit 1
}

# Check for required commands
check_cmd() {
    command -v "$1" >/dev/null 2>&1
}

need_cmd() {
    if ! check_cmd "$1"; then
        error "Required command '$1' not found. Please install it first."
    fi
}

# Detect platform
detect_platform() {
    OS=$(uname -s)
    ARCH=$(uname -m)

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64)  echo "aarch64-apple-darwin" ;;
                x86_64) echo "x86_64-apple-darwin" ;;
                *)      error "Unsupported macOS architecture: $ARCH" ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                aarch64) echo "aarch64-unknown-linux-gnu" ;;
                x86_64)  echo "x86_64-unknown-linux-gnu" ;;
                *)       error "Unsupported Linux architecture: $ARCH" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$ARCH" in
                x86_64) echo "x86_64-pc-windows-msvc" ;;
                *)      error "Unsupported Windows architecture: $ARCH" ;;
            esac
            ;;
        *)
            error "Unsupported operating system: $OS"
            ;;
    esac
}

# Get latest version from GitHub API
get_latest_version() {
    if check_cmd curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$GITHUB_API" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/'
    elif check_cmd wget; then
        wget --https-only -qO- "$GITHUB_API" | grep '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/'
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Download file
download() {
    url="$1"
    dest="$2"

    if check_cmd curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$dest"
    elif check_cmd wget; then
        wget --https-only -q "$url" -O "$dest"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Called when checksum verification cannot be performed; aborts unless the
# user explicitly opted out with HX_ALLOW_UNVERIFIED=1
checksum_unavailable() {
    reason="$1"
    if [ "$HX_ALLOW_UNVERIFIED" = "1" ]; then
        warn "$reason - proceeding WITHOUT verification (HX_ALLOW_UNVERIFIED=1)"
    else
        error "$reason. Refusing to install unverified binaries. Set HX_ALLOW_UNVERIFIED=1 to override."
    fi
}

# Determine installation directory
get_install_dir() {
    if [ -n "$HX_INSTALL_DIR" ]; then
        echo "$HX_INSTALL_DIR"
    elif [ -d "$HOME/.local/bin" ]; then
        echo "$HOME/.local/bin"
    elif [ -w "/usr/local/bin" ]; then
        echo "/usr/local/bin"
    else
        echo "$HOME/.local/bin"
    fi
}

# Check if directory is in PATH
in_path() {
    case ":$PATH:" in
        *":$1:"*) return 0 ;;
        *)        return 1 ;;
    esac
}

# Get shell config file
get_shell_config() {
    SHELL_NAME=$(basename "$SHELL")
    case "$SHELL_NAME" in
        bash)
            if [ -f "$HOME/.bashrc" ]; then
                echo "$HOME/.bashrc"
            elif [ -f "$HOME/.bash_profile" ]; then
                echo "$HOME/.bash_profile"
            else
                echo "$HOME/.profile"
            fi
            ;;
        zsh)
            echo "$HOME/.zshrc"
            ;;
        fish)
            echo "$HOME/.config/fish/config.fish"
            ;;
        *)
            echo "$HOME/.profile"
            ;;
    esac
}

# Default name, and the fallback used when `hx` is already taken.
DEFAULT_BIN_NAME="hx"
FALLBACK_BIN_NAME="hxs"

# Decide what to call the installed binary.
#
# Helix's editor binary is also called `hx` and ships in most distributions,
# so installing over it would leave whichever comes first on PATH winning and
# the other unreachable. Detect that and step aside by default, rather than
# making the user diagnose PATH order after the fact.
resolve_binary_name() {
    # Explicit choice always wins.
    if [ -n "$HX_BINARY_NAME" ]; then
        echo "$HX_BINARY_NAME"
        return
    fi

    existing=$(command -v "$DEFAULT_BIN_NAME" 2>/dev/null || true)

    # Nothing owns the name yet.
    if [ -z "$existing" ]; then
        echo "$DEFAULT_BIN_NAME"
        return
    fi

    # Our own previous install, at the directory we are about to write to.
    if [ "$existing" = "$INSTALL_DIR/$DEFAULT_BIN_NAME" ]; then
        echo "$DEFAULT_BIN_NAME"
        return
    fi

    # An hx installed somewhere else -- still an upgrade, not a collision.
    # Ask it. Anything that does not identify as hx we treat as foreign.
    if "$existing" --version 2>/dev/null | grep -qi "^hx "; then
        echo "$DEFAULT_BIN_NAME"
        return
    fi

    echo "$FALLBACK_BIN_NAME"
}

main() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --binary-name)
                [ -n "$2" ] || error "--binary-name requires a value"
                HX_BINARY_NAME="$2"
                shift 2
                ;;
            --binary-name=*)
                HX_BINARY_NAME="${1#*=}"
                [ -n "$HX_BINARY_NAME" ] || error "--binary-name requires a value"
                shift
                ;;
            *)
                error "Unknown option: $1"
                ;;
        esac
    done

    printf "\n"
    printf "${BOLD}${CYAN}hx${NC} installer\n"
    printf "\n"

    # Detect platform
    TARGET=$(detect_platform)
    info "Detected platform: $TARGET"

    # Get version
    if [ -n "$HX_VERSION" ]; then
        VERSION="$HX_VERSION"
        info "Installing specified version: v$VERSION"
    else
        info "Fetching latest version..."
        VERSION=$(get_latest_version)
        if [ -z "$VERSION" ]; then
            error "Failed to determine latest version. Set HX_VERSION manually."
        fi
        info "Latest version: v$VERSION"
    fi

    # Determine archive extension
    case "$TARGET" in
        *windows*) EXT="zip" ;;
        *)         EXT="tar.gz" ;;
    esac

    ARCHIVE="hx-v${VERSION}-${TARGET}.${EXT}"
    URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARCHIVE}"
    CHECKSUM_URL="${URL}.sha256"

    # Create temp directory
    TMPDIR=$(mktemp -d)
    trap "rm -rf $TMPDIR" EXIT

    info "Downloading hx v$VERSION..."
    if ! download "$URL" "$TMPDIR/$ARCHIVE" 2>/dev/null; then
        error "Failed to download $ARCHIVE. Check if the release exists."
    fi

    # Verify checksum (mandatory unless HX_ALLOW_UNVERIFIED=1)
    if check_cmd sha256sum; then
        info "Verifying checksum..."
        if download "$CHECKSUM_URL" "$TMPDIR/$ARCHIVE.sha256" 2>/dev/null; then
            (cd "$TMPDIR" && sha256sum -c "$ARCHIVE.sha256" >/dev/null 2>&1) || \
                error "Checksum verification failed"
        else
            checksum_unavailable "Checksum file not found at $CHECKSUM_URL"
        fi
    elif check_cmd shasum; then
        info "Verifying checksum..."
        if download "$CHECKSUM_URL" "$TMPDIR/$ARCHIVE.sha256" 2>/dev/null; then
            EXPECTED=$(cut -d' ' -f1 "$TMPDIR/$ARCHIVE.sha256")
            ACTUAL=$(shasum -a 256 "$TMPDIR/$ARCHIVE" | cut -d' ' -f1)
            if [ "$EXPECTED" != "$ACTUAL" ]; then
                error "Checksum verification failed"
            fi
        else
            checksum_unavailable "Checksum file not found at $CHECKSUM_URL"
        fi
    else
        checksum_unavailable "Neither sha256sum nor shasum is available"
    fi

    # Extract
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
    BINARY=$(find "$TMPDIR" -type f \( -name "hx" -o -name "hx.exe" \) | head -n1)
    if [ -z "$BINARY" ]; then
        error "Binary not found in archive"
    fi

    # Install binary
    INSTALL_DIR=$(get_install_dir)

    BIN_NAME=$(resolve_binary_name)
    if [ "$BIN_NAME" != "$DEFAULT_BIN_NAME" ]; then
        EXISTING=$(command -v "$DEFAULT_BIN_NAME" 2>/dev/null || true)
        warn "\`$DEFAULT_BIN_NAME\` on this system already refers to $EXISTING"
        info "Installing as \`$BIN_NAME\` so both stay reachable"
        printf "      Override with ${CYAN}HX_BINARY_NAME=$DEFAULT_BIN_NAME${NC} to take the name anyway.\n"
    fi

    info "Installing to $INSTALL_DIR as $BIN_NAME..."

    mkdir -p "$INSTALL_DIR" 2>/dev/null || true

    if [ -w "$INSTALL_DIR" ]; then
        cp "$BINARY" "$INSTALL_DIR/$BIN_NAME"
        chmod +x "$INSTALL_DIR/$BIN_NAME"
    else
        info "Requesting sudo access..."
        sudo mkdir -p "$INSTALL_DIR"
        sudo cp "$BINARY" "$INSTALL_DIR/$BIN_NAME"
        sudo chmod +x "$INSTALL_DIR/$BIN_NAME"
    fi

    # Success message
    printf "\n"
    printf "${GREEN}${BOLD}hx v$VERSION installed successfully as $BIN_NAME!${NC}\n"
    printf "\n"

    # PATH instructions
    if ! in_path "$INSTALL_DIR"; then
        if [ -z "$HX_NO_MODIFY_PATH" ]; then
            SHELL_CONFIG=$(get_shell_config)
            warn "$INSTALL_DIR is not in your PATH"
            printf "\n"
            printf "Add it to your shell config:\n"
            printf "\n"
            printf "  ${CYAN}echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> $SHELL_CONFIG${NC}\n"
            printf "  ${CYAN}source $SHELL_CONFIG${NC}\n"
            printf "\n"
        fi
    fi

    # Next steps
    printf "Get started:\n"
    printf "\n"
    printf "  ${CYAN}$BIN_NAME --help${NC}              Show available commands\n"
    printf "  ${CYAN}$BIN_NAME init myproject${NC}      Create a new Haskell project\n"
    printf "  ${CYAN}$BIN_NAME doctor${NC}              Check your Haskell setup\n"
    printf "  ${CYAN}$BIN_NAME completions install${NC} Install shell completions\n"
    printf "\n"

    # Verify installation worked
    if in_path "$INSTALL_DIR" && check_cmd "$BIN_NAME"; then
        info "Run '$BIN_NAME --version' to verify: $("$BIN_NAME" --version 2>/dev/null || echo 'installed')"
    fi
}

main "$@"
