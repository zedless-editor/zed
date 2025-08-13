{
  lib,
  rustPlatform,
  cmake,
  copyDesktopItems,
  curl,
  perl,
  pkg-config,
  protobuf,
  fontconfig,
  freetype,
  libgit2,
  openssl,
  sqlite,
  zlib,
  zstd,
  alsa-lib,
  libxkbcommon,
  wayland,
  libglvnd,
  xorg,
  stdenv,
  makeFontsConf,
  vulkan-loader,
  envsubst,
  gitUpdater,
  cargo-about,
  versionCheckHook,
  zed-editor,
  cargo-bundle,
  git,
  apple-sdk_15,
  darwinMinVersionHook,
  libGL,
  libX11,
  libXext,
  livekit-libwebrtc,
  testers,
  writableTmpDirAsHomeHook,
  withGLES ? false,
  buildRemoteServer ? true,
  buildZetaCli ? true,
}:
assert withGLES -> stdenv.hostPlatform.isLinux; let
  inherit (builtins) fromTOML readFile;
  inherit (lib.fileset) toSource unions;

  gpu-lib =
    if withGLES
    then libglvnd
    else vulkan-loader;

  # Cargo.toml located in repo root does not contain any version information.
  cargoToml = fromTOML (readFile ../crates/zed/Cargo.toml);
  pname = cargoToml.package.name;
  version = cargoToml.package.version;
  src = toSource {
    root = ../.;
    fileset = unions [
      ../crates
      ../assets
      ../extensions
      ../script
      ../tooling
      ../Cargo.toml
      ../Cargo.lock
      ../.config
    ];
  };
in
  rustPlatform.buildRustPackage {
    inherit pname version src;

    outputs = ["out"] ++ lib.optional buildRemoteServer "remote_server";

    # Dynamically link WebRTC instead of static
    postPatch = ''
      substituteInPlace ../cargo-vendor-dir/webrtc-sys-*/build.rs \
        --replace-fail "cargo:rustc-link-lib=static=webrtc" "cargo:rustc-link-lib=dylib=webrtc"

      echo stable > crates/zed/RELEASE_CHANNEL
    '';

    cargoDeps = rustPlatform.importCargoLock {
      lockFile = ../Cargo.lock;
      outputHashes = import ./cargo-hashes.nix;
    };

    nativeBuildInputs =
      [
        cmake
        copyDesktopItems
        curl
        perl
        pkg-config
        protobuf
        rustPlatform.bindgenHook
        cargo-about
      ]
      ++ lib.optionals stdenv.hostPlatform.isDarwin [cargo-bundle];

    dontUseCmakeConfigure = true;

    buildInputs =
      [
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
        # required by livekit:
        libGL
        libX11
        libXext
      ]
      ++ lib.optionals stdenv.hostPlatform.isDarwin [
        apple-sdk_15
        # ScreenCaptureKit, required by livekit, is only available on 12.3 and up:
        # https://developer.apple.com/documentation/screencapturekit
        (darwinMinVersionHook "12.3")
      ];

    cargoBuildFlags =
      [
        "--package=zed"
        "--package=cli"
      ]
      ++ lib.optional buildRemoteServer "--package=remote_server"
      ++ lib.optional buildZetaCli "--package=zeta_cli";

    # Required on darwin because we don't have access to the
    # proprietary Metal shader compiler.
    buildFeatures = lib.optionals stdenv.hostPlatform.isDarwin ["gpui/runtime_shaders"];

    env = {
      RUSTFLAGS = lib.concatStringsSep " " ([
        "-C link-arg=-Wl,-rpath,${lib.makeLibraryPath [ wayland gpu-lib ]}"
      ] ++ lib.optional withGLES "--cfg gles");
      ALLOW_MISSING_LICENSES = true;
      ZSTD_SYS_USE_PKG_CONFIG = true;
      FONTCONFIG_FILE = makeFontsConf {
        fontDirectories = [
          "${src}/assets/fonts/plex-mono"
          "${src}/assets/fonts/plex-sans"
        ];
      };
      # Setting this environment variable allows to disable auto-updates
      # https://zed.dev/docs/development/linux#notes-for-packaging-zed
      ZED_UPDATE_EXPLANATION = "Zed has been installed using Nix. Auto-updates have thus been disabled.";
      # Used by `zed --version`
      RELEASE_VERSION = version;
      LK_CUSTOM_WEBRTC = livekit-libwebrtc;
    };

    preBuild = ''
      bash script/generate-licenses
    '';

    postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
      patchelf --add-rpath ${gpu-lib}/lib $out/libexec/*
      patchelf --add-rpath ${wayland}/lib $out/libexec/*
    '';

    nativeCheckInputs = [
      writableTmpDirAsHomeHook
    ];

    checkFlags =
      [
        # Flaky: unreliably fails on certain hosts (including Hydra)
        "--skip=zed::tests::test_window_edit_state_restoring_enabled"
      ]
      ++ lib.optionals stdenv.hostPlatform.isLinux [
        # Fails on certain hosts (including Hydra) for unclear reason
        "--skip=test_open_paths_action"
      ];

    installPhase =
      ''
        runHook preInstall

        release_target="target/${stdenv.hostPlatform.rust.cargoShortTarget}/release"
      ''
      + lib.optionalString stdenv.hostPlatform.isDarwin ''
        # cargo-bundle expects the binary in target/release
        mv $release_target/zed target/release/zed

        pushd crates/zed

        # Note that this is GNU sed, while Zed's bundle-mac uses BSD sed
        sed -i "s/package.metadata.bundle-stable/package.metadata.bundle/" Cargo.toml
        export CARGO_BUNDLE_SKIP_BUILD=true
        app_path=$(cargo bundle --release | xargs)

        # We're not using Zed's fork of cargo-bundle, so we must manually append their plist extensions
        # Remove closing tags from Info.plist (last two lines)
        head -n -2 $app_path/Contents/Info.plist > Info.plist
        # Append extensions
        cat resources/info/*.plist >> Info.plist
        # Add closing tags
        printf "</dict>\n</plist>\n" >> Info.plist
        mv Info.plist $app_path/Contents/Info.plist

        popd

        mkdir -p $out/Applications $out/bin
        # Zed expects git next to its own binary
        ln -s ${lib.getExe git} $app_path/Contents/MacOS/git
        mv $release_target/cli $app_path/Contents/MacOS/cli
        mv $app_path $out/Applications/

        # Physical location of the CLI must be inside the app bundle as this is used
        # to determine which app to start
        ln -s $out/Applications/Zed.app/Contents/MacOS/cli $out/bin/zedless
        ln -s $out/Applications/Zed.app/Contents/MacOS/cli $out/bin/zed
        ln -s $out/Applications/Zed.app/Contents/MacOS/cli $out/bin/zeditor
      ''
      + lib.optionalString stdenv.hostPlatform.isLinux ''
        install -Dm755 $release_target/zed $out/libexec/zed-editor
        install -Dm755 $release_target/cli $out/bin/zedless
        ln -s $out/bin/zedless $out/bin/zed
        ln -s $out/bin/zedless $out/bin/zeditor

        install -Dm644 ${src}/crates/zed/resources/app-icon@2x.png $out/share/icons/hicolor/1024x1024/apps/zedless.png
        install -Dm644 ${src}/crates/zed/resources/app-icon.png $out/share/icons/hicolor/512x512/apps/zedless.png

        # extracted from https://github.com/zed-industries/zed/blob/v0.141.2/script/bundle-linux (envsubst)
        # and https://github.com/zed-industries/zed/blob/v0.141.2/script/install.sh (final desktop file name)
        (
          export DO_STARTUP_NOTIFY="true"
          export APP_CLI="zedless"
          export APP_ICON="zedless"
          export APP_NAME="Zedless"
          export APP_ARGS="%U"
          mkdir -p "$out/share/applications"
          ${lib.getExe envsubst} < "crates/zed/resources/zed.desktop.in" > "$out/share/applications/cooking.schizo.Zedless.desktop"
        )
      ''
      + lib.optionalString buildRemoteServer ''
        install -Dm755 $release_target/remote_server $remote_server/bin/zed-remote-server-stable-$version
      ''
      + lib.optionalString buildZetaCli ''
        install -Dm755 $release_target/zeta $out/bin/zeta
      ''
      + ''
        runHook postInstall
      '';

    nativeInstallCheckInputs = [
      versionCheckHook
    ];
    versionCheckProgram = "${placeholder "out"}/bin/zedless";
    versionCheckProgramArg = ["--version"];
    doInstallCheck = true;

    passthru = {
      tests =
        {
          remoteServerVersion = testers.testVersion {
            package = zed-editor.remote_server;
            command = "zed-remote-server-stable-${version} version";
          };
        }
        // lib.optionalAttrs stdenv.hostPlatform.isLinux {
          withGles = zed-editor.override {withGLES = true;};
        };
    };

    meta = {
      description = "High-performance, multiplayer code editor from the creators of Atom and Tree-sitter";
      homepage = "https://github.com/zedless-editor/zed";
      license = lib.licenses.gpl3Only;
      maintainers = with lib.maintainers; [
        max
        NotAShelf
      ];
      mainProgram = "zedless";
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
    };
  }
