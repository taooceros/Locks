cmake_minimum_required(VERSION 3.22)

project(locks)

set(CMAKE_C_FLAGS "-pthread -g -Wall -O2")
set(CMAKE_C_FLAGS_DEBUG "-pthread -g -Wall")
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY bin)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

add_compile_definitions(CYCLE_PER_US=2700)

include_directories(CCsynch FlatCombining FlatCombiningFair)
include_directories(shared)
include_directories(RCL)

add_executable(example example.c
        FlatCombining/flatcombining.c CCsynch/ccsynch.c RCL/rcl.c shared/lockfree_stack.c shared/lockfree_stack.h FlatCombiningFair/flatcombiningfair.c)
add_executable(test
        shared/lockfree_stack.c
        unit_test/lockfree_stack_test.c
        "unit_test/unit_test.c"
        "unit_test/lock_tester.c"
        FlatCombining/flatcombining.c
        CCsynch/ccsynch.c RCL/rcl.c RCL/rcl.h FlatCombiningFair/flatcombiningfair.c)



add_executable(usclexample proportional-share/u-scl/example/main.c shared/lockfree_stack.c shared/lockfree_stack.h)

add_compile_definitions(FAIRLOCK)