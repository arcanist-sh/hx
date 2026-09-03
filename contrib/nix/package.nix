{
  lib,
  rustPlatform,
  fetchFromGitHub,
  pkg-config,
  openssl,
  stdenv,
  darwin,
  installShellFiles,
}:

rustPlatform.buildRustPackage rec {
  pname = "hx";
  version = "0.4.1";

  src = fetchFromGitHub {
    owner = "arcanist-sh";
    repo = "hx";
    rev = "v${version}";
    hash = "sha256-HM6mEweF62t4+IyoHBgrVRkRpZmG2ibonJrH+wG3koc=";
  };

  cargoHash = lib.fakeHash; # TODO: compute with nix build

  nativeBuildInputs = [
    pkg-config
    installShellFiles
  ];

  buildInputs =
    [ openssl ]
    ++ lib.optionals stdenv.hostPlatform.isDarwin [
      darwin.apple_sdk.frameworks.Security
      darwin.apple_sdk.frameworks.SystemConfiguration
    ];

  postInstall = lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    installShellCompletion --cmd hx \
      --bash <($out/bin/hx completions generate bash) \
      --fish <($out/bin/hx completions generate fish) \
      --zsh <($out/bin/hx completions generate zsh)
  '';

  meta = {
    description = "Fast, opinionated Haskell toolchain CLI";
    homepage = "https://github.com/arcanist-sh/hx";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ ];
    mainProgram = "hx";
  };
}
