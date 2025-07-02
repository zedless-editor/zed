{
  mkShell,
  makeFontsConf,

  zed-editor,

  rust-analyzer,
  cargo-nextest,
  cargo-hakari,
  cargo-machete,
  nixfmt-rfc-style,
  rustfmt,
  protobuf,
}:
(mkShell.override { inherit (zed-editor) stdenv; }) {
  inputsFrom = [ zed-editor ];
  packages = [
    rust-analyzer
    cargo-nextest
    cargo-hakari
    cargo-machete
    nixfmt-rfc-style
    rustfmt
  ];

  env =
    let
      baseEnvs =
        (zed-editor.overrideAttrs (attrs: {
          passthru = { inherit (attrs) env; };
        })).env; # exfil `env`; it's not in drvAttrs
    in
    (removeAttrs baseEnvs [
      "LK_CUSTOM_WEBRTC" # download the staticlib during the build as usual
      "CARGO_PROFILE" # let you specify the profile
      "TARGET_DIR"
    ])
    // {
      # note: different than `$FONTCONFIG_FILE` in `build.nix` – this refers to relative paths
      # outside the nix store instead of to `$src`
      FONTCONFIG_FILE = makeFontsConf {
        fontDirectories = [
          "./assets/fonts/plex-mono"
          "./assets/fonts/plex-sans"
        ];
      };
      PROTOC = "${protobuf}/bin/protoc";
    };
}
