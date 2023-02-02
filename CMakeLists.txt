cmake_minimum_required(VERSION 3.22)

project(locks)

set(CMAKE_C_FLAGS -pthread)
set(CMAKE_C_FLAGS -g)
add_compile_definitions(CYCLE_PER_US=2700)

add_executable(example example.c
        FlatCombining/flatcombining.c CCsynch/ccsynch.c)

include_directories(proportional-share/u-scl)
include_directories(CCsynch FlatCombining)

add_executable(usclexample proportional-share/u-scl/example/main.c)

add_compile_definitions(FAIRLOCK)