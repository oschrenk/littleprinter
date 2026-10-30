{
  description = "Little Printer tooling: payload encoder for the 2012 BERG Little Printer";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The rustc in nixpkgs carries std for the host only. rust-overlay adds
    # rust-std for the Bridge target from the official dist channel, which is
    # the whole reason this input exists.
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # The BERG Bridge is armv5tejl, ARM926EJ-S, with no vfp in
        # /proc/cpuinfo, so soft float is mandatory. Static musl bypasses the
        # device's uClibc 0.9.33.2 entirely, the same reason the Go spike used
        # CGO_ENABLED=0. Rust documents a 3.2 kernel floor and the Bridge runs
        # 2.6.36.4, below it; the encoder is measured to work there anyway.
        bridgeTarget = "armv5te-unknown-linux-musleabi";

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ bridgeTarget ];
        };
      in
      {
        formatter = pkgs.nixfmt-tree;

        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.git-lfs
            pkgs.go-task
          ];

          # rust-lld does the cross link. rust-std for the target ships
          # self-contained crt objects and libc.a, so no cross gcc and no
          # uClibc sysroot are needed. Set here rather than in
          # client/.cargo/config.toml so no store path is committed.
          CARGO_TARGET_ARMV5TE_UNKNOWN_LINUX_MUSLEABI_LINKER = "${rustToolchain}/lib/rustlib/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/bin/rust-lld";
          CARGO_TARGET_ARMV5TE_UNKNOWN_LINUX_MUSLEABI_RUSTFLAGS = "-C linker-flavor=ld.lld";

          BRIDGE_TARGET = bridgeTarget;
        };
      }
    );
}
