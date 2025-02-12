{
  pkgs ? import <nixpkgs> { },
}:
let
  inherit (pkgs) lib;
in
pkgs.mkShell.override { stdenv = pkgs.useMoldLinker pkgs.clangStdenv; } {
  packages =
    [
      pkgs.curl
      pkgs.cmake
      pkgs.perl
      pkgs.pkg-config
      pkgs.protobuf
      pkgs.rustPlatform.bindgenHook
      pkgs.rust-analyzer
    ];

  buildInputs =
    [
      pkgs.bzip2
      pkgs.curl
      pkgs.fontconfig
      pkgs.freetype
      pkgs.libgit2
      pkgs.openssl
      pkgs.sqlite
      pkgs.zlib
      pkgs.zstd
      pkgs.rustToolchain
    ]
    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
      pkgs.alsa-lib
      pkgs.libxkbcommon
      pkgs.wayland
      pkgs.xorg.libxcb
      pkgs.vulkan-loader
    ]
    ++ lib.optional pkgs.stdenv.hostPlatform.isDarwin pkgs.apple-sdk_15;

  PROTOC="${pkgs.protobuf}/bin/protoc";

  ZSTD_SYS_USE_PKG_CONFIG = true;
}
