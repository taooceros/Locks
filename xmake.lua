add_rules("mode.debug", "mode.release")

if is_mode("debug") then
    add_defines("DEBUG")
end

add_cflags("-pthread")

add_includedirs("FlatCombining/")
add_includedirs("FlatCombiningFair/")
add_includedirs("CCsynch/")
add_includedirs("RCL/")
add_includedirs("shared")
add_defines("CYCLE_PER_US=2400")

add_files("shared/*.c")
add_files("FlatCombining/*.c")
add_files("FlatCombiningFair/*.c")
add_files("CCsynch/*.c")
add_files("RCL/*.c")

target("example")
    set_kind("binary")
    add_files("example.c")
    


target("lock_test")
    set_kind("binary")
    add_files("unit_test/*.c")