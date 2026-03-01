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
    linuxPackages.perf
  ];

  languages.python = {
    enable = true;
    venv.enable = true;
    uv.enable = true;
    venv.requirements = ''
      pyarrow
    '';
  };

  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

  languages.rust = {
    enable = true;
    channel = "nightly";
  };
}
