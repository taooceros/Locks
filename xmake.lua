add_rules("mode.debug", "mode.release")

add_links("pthread")

set_toolset("cc", "clang")

if is_mode("debug") then
    add_defines("DEBUG")
    set_optimize("fastest")
end
if is_mode("release") then
    set_optimize("faster")
end


add_includedirs("FlatCombining/original")
add_includedirs("FlatCombining/fair_ban")
add_includedirs("FlatCombining/fair_pq")
add_includedirs("CCsynch/")
add_includedirs("RCL/")
add_includedirs("ticket/")
add_includedirs("shared")
add_includedirs("libpqueue/src")
add_defines("CYCLE_PER_US=2400",
            "FC_THREAD_MAX_CYCLE=CYCLE_PER_MS",
            "_GNU_SOURCE")

add_files("shared/*.c", 
          "libpqueue/src/pqueue.c",
          "FlatCombining/**/*.c",
          "CCsynch/*.c",
          "RCL/*.c",
          "ticket/*.c")

target("example")
    set_kind("binary")
    add_files("example.c")
    set_targetdir("bin")
    set_arch("x86_64")


target("lock_test")
    set_kind("binary")
    add_files("unit_test/*.c")
    set_targetdir("tests")
    set_arch("x86_64")
