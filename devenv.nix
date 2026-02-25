{ pkgs, lib, ... }:

{
  packages = [
    pkgs.clang
    pkgs.llvmPackages.libclang
    pkgs.pkg-config
    pkgs.mold-wrapped
    pkgs.just
  ];

  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

  languages.rust = {
    enable = true;
    channel = "nightly";
  };
}
