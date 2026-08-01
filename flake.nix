{
  description = "hx - Fast, opinionated Haskell toolchain CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    # nixpkgs 26.11 dropped x86_64-darwin support, which broke evaluation of
    # this flake (`nix flake show --all-systems`, as run by flakehub-push).
    # Enumerate the supported systems explicitly and omit x86_64-darwin. The
    # prebuilt x86_64-darwin binary is still published as a release artifact;
    # only the Nix package drops that platform.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        version = "0.9.1";

        # Binary releases for each platform
        sources = {
          x86_64-linux = {
            url = "https://github.com/arcanist-sh/hx/releases/download/v${version}/hx-v${version}-x86_64-unknown-linux-gnu.tar.gz";
            sha256 = "f005e671266933f7655aca92ed7125534a9fe537cd640eba87ca119adb3bc288";
          };
          aarch64-linux = {
            url = "https://github.com/arcanist-sh/hx/releases/download/v${version}/hx-v${version}-aarch64-unknown-linux-gnu.tar.gz";
            sha256 = "24efcd2c4e7596c70a8759eeb7a116e13d8834e4e4b9929b84fd704f62b73e11";
          };
          aarch64-darwin = {
            url = "https://github.com/arcanist-sh/hx/releases/download/v${version}/hx-v${version}-aarch64-apple-darwin.tar.gz";
            sha256 = "a73e90d7477fa8162bbc5e1c293b272407c624f3aedb4ec78989dd2a70dbca33";
          };
        };

        src = sources.${system} or (throw "Unsupported system: ${system}");
      in
      {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "hx";
          inherit version;

          src = pkgs.fetchurl {
            inherit (src) url sha256;
          };

          sourceRoot = ".";

          nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.autoPatchelfHook
          ];

          buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.stdenv.cc.cc.lib
          ];

          installPhase = ''
            runHook preInstall
            install -Dm755 hx $out/bin/hx
            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "Fast, opinionated Haskell toolchain CLI";
            homepage = "https://github.com/arcanist-sh/hx";
            license = licenses.mit;
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
            mainProgram = "hx";
          };
        };

        packages.hx = self.packages.${system}.default;
      }
    );
}
