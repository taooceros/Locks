add_rules("plugin.compile_commands.autoupdate", { outputdir = ".vscode" })
add_rules("mode.debug", "mode.release")

add_requires("cargo::dlock", {configs = {cargo_toml = path.join(os.projectdir(), "rust/Cargo.toml"), version="*"}})

set_toolset("cc", "gcc")
-- set_toolset("rcld", "mold")


add_rcflags("--cfg=feature=\"combiner_stat\"", "-C link-arg=-fuse-ld=mold", {force=true})

if is_mode("debug") then
    add_defines("DEBUG")
    set_warnings("all")
end
if is_mode("release") then
    set_optimize("faster")
    set_warnings("all")
end


add_includedirs("c/FlatCombining/original")
add_includedirs("c/FlatCombining/fair_ban")
add_includedirs("c/FlatCombining/fair_pq")
add_includedirs("c/u-scl/")
add_includedirs("c/CCsynch/")
add_includedirs("c/RCL/")
add_includedirs("c/ticket/")
add_includedirs("c/shared")
add_includedirs("c/libpqueue/src")
add_defines("CYCLE_PER_US=2400",
    "FC_THREAD_MAX_CYCLE=CYCLE_PER_MS",
    "_GNU_SOURCE")

add_files("c/shared/*.c",
    "c/libpqueue/src/pqueue.c",
    "c/FlatCombining/**/*.c",
    "c/CCsynch/*.c",
    "c/RCL/*.c",
    "c/ticket/*.c",
    "c/u-scl/*.c")

target("example")
    set_kind("binary")
    add_links("pthread")
    add_files("c/example.c")
    set_targetdir("bin")
    set_default(false)
target_end()


target("lock_test")
    set_kind("binary")
    add_links("pthread")
    add_files("c/unit_test/*.c")
    set_targetdir("tests")
    set_default(false)
target_end()

target("cdlocks")
    add_links("pthread")
    set_kind("static")
target_end()

target("dlock")
    set_values("rust.edition", "2021")
    set_kind("binary")
    set_arch("x86_64-unknown-linux-gnu")
    add_deps("cdlocks")
    add_files("rust/src/main.rs")
    add_packages("cargo::dlock")
    set_rundir("$(projectdir)")
target_end()