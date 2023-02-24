add_rules("mode.debug", "mode.release")

add_links("pthread")

set_toolset("cc", "/usr/bin/gcc-12")


if is_mode("debug") then
    add_defines("DEBUG")
    add_cflags("-g", "-pthread")
else 
    add_cflags("-g", "-pthread")
end


add_includedirs("FlatCombining/original")
add_includedirs("FlatCombining/fair_ban")
add_includedirs("FlatCombining/fair_pq")
add_includedirs("CCsynch/")
add_includedirs("RCL/")
add_includedirs("shared")
add_defines("CYCLE_PER_US=2400")

add_files("shared/*.c")
add_files("FlatCombining/**/*.c")
add_files("CCsynch/*.c")
add_files("RCL/*.c")

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
