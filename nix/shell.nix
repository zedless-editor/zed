{
  lib,
  mkShell,
  stdenv,
  useMoldLinker,
  clangStdenv,
  curl,
  cmake,
  perl,
  pkg-config,
  protobuf,
  rustPlatform,
  rust-analyzer,
  bzip2,
  fontconfig,
  freetype,
  libgit2,
  openssl,
  sqlite,
  zlib,
  zstd,
  cargo,
  alsa-lib,
  libxkbcommon,
  wayland,
  xorg,
  vulkan-loader,
  apple-sdk_15,
  ...
}:
mkShell.override {stdenv = useMoldLinker clangStdenv;} {
  packages = [
    curl
    cmake
    perl
    pkg-config
    protobuf
    rustPlatform.bindgenHook
    rust-analyzer
    cargo
  ];

  buildInputs =
    [
      bzip2
      curl
      fontconfig
      freetype
      libgit2
      openssl
      sqlite
      zlib
      zstd
    ]
    ++ lib.optionals stdenv.hostPlatform.isLinux [
      alsa-lib
      libxkbcommon
      wayland
      xorg.libxcb
      vulkan-loader
    ]
    ++ lib.optional stdenv.hostPlatform.isDarwin apple-sdk_15;

  PROTOC = "${protobuf}/bin/protoc";

  ZSTD_SYS_USE_PKG_CONFIG = true;
}
