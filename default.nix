{ lib
, rustPlatform
, curl
, makeWrapper
}:

rustPlatform.buildRustPackage rec {
  pname = "grok-statusline";
  version = "0.3.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  postInstall = ''
    wrapProgram $out/bin/grok-statusline --prefix PATH : ${lib.makeBinPath [ curl ]}
  '';

  meta = with lib; {
    description = "Claude-style bottom statusline for Grok Build";
    homepage = "https://github.com/Sim-hu/grok-statusline";
    license = licenses.mit;
    mainProgram = "grok-statusline";
    platforms = platforms.unix;
  };
}
