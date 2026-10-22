{ pkgs, lib, inputs, ... }:
let
  inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;

  rustBin = inputs.rust-overlay.lib.mkRustBin { } pkgs.buildPackages;
  toolchainFrom = file: rustBin.fromRustupToolchainFile file;

  # rustfmt runs on the nightly in tools/rustfmt for its unstable options. Only rustfmt and
  # cargo-fmt are exposed, so that toolchain's cargo and rustc can't shadow the pinned ones.
  rustfmt = pkgs.runCommand "rustfmt-nightly" { } ''
    mkdir -p $out/bin
    ln -s ${toolchainFrom ./tools/rustfmt/rust-toolchain.toml}/bin/{rustfmt,cargo-fmt} $out/bin/
  '';

  # Hawk uses rustc_private, so it only runs on the exact toolchain it was built against,
  # pinned in tools/hawk. The wrapper hands it that toolchain whichever cargo invokes it.
  hawkToolchain = toolchainFrom ./tools/hawk/rust-toolchain.toml;
  # cargo-hawk points the dynamic loader at the toolchain's libraries so its driver can find
  # librustc_driver; the driver gets an rpath below instead. Left set, the variable reaches
  # Nix's clang when cargo links build scripts and loads rustc's LLVM into it. (Apple's cc
  # is SIP-protected and never sees it, which is why rustup setups don't hit this.)
  hawkCargo = pkgs.writeShellScriptBin "cargo" ''
    unset DYLD_LIBRARY_PATH LD_LIBRARY_PATH
    exec ${hawkToolchain}/bin/cargo "$@"
  '';
  hawk = pkgs.stdenv.mkDerivation (finalAttrs: {
    pname = "cargo-hawk";
    # Keep paired with tools/hawk/rust-toolchain.toml.
    version = "0.1.14";
    src = pkgs.fetchurl (
      let
        release = target: hash: {
          url = "https://github.com/astral-sh/hawk/releases/download/${finalAttrs.version}/cargo-hawk-${target}.tar.gz";
          inherit hash;
        };
      in
      {
        aarch64-darwin = release "aarch64-apple-darwin" "sha256-oTeWICRbpW1TvKNyf1ex25WbYIW0pk3EKlbxI2f9ZMY=";
        x86_64-linux = release "x86_64-unknown-linux-gnu" "sha256-bMi1fg2R7b33A4glTHPi3PdlOSnYMIHkfcKm1shbA2k=";
        aarch64-linux = release "aarch64-unknown-linux-gnu" "sha256-KjbtYkSntkatoMfpvJDLwXV/F8xK9mdeyzLuwp193iM=";
      }.${pkgs.stdenv.hostPlatform.system}
    );
    nativeBuildInputs = [ pkgs.makeWrapper ] ++ lib.optional isLinux pkgs.autoPatchelfHook;
    buildInputs = lib.optionals isLinux [ hawkToolchain pkgs.stdenv.cc.cc.lib ];
    installPhase = ''
      install -Dm755 -t $out/bin cargo-hawk cargo-hawk-driver
    '' + lib.optionalString isDarwin ''
      install_name_tool -add_rpath ${hawkToolchain}/lib $out/bin/cargo-hawk-driver
    '' + ''
      wrapProgram $out/bin/cargo-hawk \
        --prefix PATH : ${hawkCargo}/bin:${hawkToolchain}/bin \
        --set CARGO ${hawkCargo}/bin/cargo \
        --set RUSTC ${hawkToolchain}/bin/rustc
    '';
    dontStrip = true;
  });

  # Linked at build time and loaded at runtime; GPUI dlopens the display and GPU libraries.
  linuxLibs = with pkgs; [
    alsa-lib
    fontconfig
    freetype
    glib
    libpulseaudio
    libva
    libx11
    libxcb
    libxkbcommon
    openssl
    vulkan-loader
    wayland
    zstd
  ];
in
{
  # On the Mac, Xcode compiles and links, not Nix's clang and Apple SDK: transcribe.cpp uses
  # Metal APIs newer than Nix's SDK, GPUI compiles its shaders with Xcode's metal,
  # tools/build-macos-app.sh compiles the app icon with Xcode's actool, and the app must not
  # link libraries from /nix/store. The Rust toolchain still puts Nix's clang on PATH, so the
  # compilers are set in env below.
  apple.sdk = null;

  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
    lsp.package = rustBin.stable.${(lib.importTOML ./rust-toolchain.toml).toolchain.channel}.rust-analyzer;
  };

  packages = [
    hawk
    rustfmt
    pkgs.cmake # transcribe.cpp
    pkgs.just
    pkgs.nodejs_24 # pi-extension tests, and prek's ast-grep hook
    pkgs.prek
    pkgs.uv # prek's Python hooks
    pkgs.zizmor
  ] ++ lib.optionals isLinux ([
    pkgs.pkg-config
    # transcribe.cpp's Vulkan backend compiles its shaders and needs headers 1.3.261+
    pkgs.shaderc
    pkgs.spirv-headers
    pkgs.vulkan-headers
  ] ++ linuxLibs);

  # Here, not in env: the compiler wrapper's setup hook sets CC and CXX after env.
  enterShell = lib.optionalString isDarwin ''
    export CC=/usr/bin/cc CXX=/usr/bin/c++
  '';

  # `devenv test`: everything CI checks
  enterTest = ''
    just fmt --check
    just check
    cargo clippy --locked --all-targets --all-features -- -D warnings
    just hawk
    just test
    prek run --all-files --skip cargo-fmt --skip cargo-clippy
  '';

  env = {
    # editors' format-on-save goes through rust-analyzer, which honors this too
    RUSTFMT = "${rustfmt}/bin/rustfmt";
  } // lib.optionalAttrs isDarwin {
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = "/usr/bin/cc";
  } // lib.optionalAttrs isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath linuxLibs;
  };
}
