{ pkgs, lib, ... }:

{
  packages = with pkgs; [
    clang
    llvmPackages.libclang
    pkg-config
    mold-wrapped
    just
    uv
    numactl
    git
    util-linux
    linuxPackages.perf
  ];

  languages.python = {
    enable = true;
    venv.enable = true;
    uv.enable = true;
    venv.requirements = ''
      pyarrow
      matplotlib==3.11.2
    '';
  };

  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

  languages.rust = {
    enable = true;
    channel = "nightly";
  };
}
