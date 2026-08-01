{
  description = "hx - Fast, opinionated Haskell toolchain CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    # nixpkgs 26.11 dropped x86_64-darwin support, so it is intentionally
    # omitted from the supported systems.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # Build hx from source rather than fetching prebuilt release tarballs.
        # The version is read from Cargo.toml and dependencies are vendored from
        # the committed Cargo.lock, so the flake never needs a per-release
        # version/hash bump — it always matches the checked-out revision.
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "hx";
          inherit version;

          src = self;

          # Every dependency is a crates.io registry crate, so the lockfile
          # alone pins them deterministically — no manual output hashes.
          cargoLock.lockFile = ./Cargo.lock;

          # Some transitive crates probe for system libraries via pkg-config;
          # ring (0.17) and zstd-sys compile their bundled C with the stdenv cc.
          nativeBuildInputs = [ pkgs.pkg-config ];

          # libiconv is needed when linking on Darwin; harmless elsewhere.
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

          # The workspace's tests shell out to cabal/ghc and touch the network,
          # neither of which exists in the Nix sandbox. CI runs the full suite;
          # the Nix build just produces the binary.
          doCheck = false;

          meta = with pkgs.lib; {
            description = "Fast, opinionated Haskell toolchain CLI";
            homepage = "https://github.com/arcanist-sh/hx";
            license = licenses.mit;
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
            mainProgram = "hx";
          };
        };

        packages.hx = self.packages.${system}.default;

        apps.default = flake-utils.lib.mkApp {
          drv = self.packages.${system}.default;
          name = "hx";
        };
      }
    );
}
