cmake_minimum_required(VERSION 3.22)

project(locks)

set(CMAKE_C_FLAGS "-pthread -g -Wall")
set(CMAKE_C_FLAGS_DEBUG "-pthread -g -Wall")

add_compile_definitions(CYCLE_PER_US=2700)

include_directories(proportional-share/u-scl)
include_directories(CCsynch FlatCombining)
include_directories(shared)

add_executable(example example.c
        FlatCombining/flatcombining.c CCsynch/ccsynch.c RCL/rcl.c shared/lockfree_stack.c shared/lockfree_stack.h)
add_executable(test
        "unit_test/unit_test.c"
        "unit_test/lock_tester.c"
        FlatCombining/flatcombining.c
        CCsynch/ccsynch.c RCL/rcl.c RCL/rcl.h shared/lockfree_stack.c shared/lockfree_stack.h unit_test/lockfree_stack_test.c unit_test/lockfree_stack_test.h)



add_executable(usclexample proportional-share/u-scl/example/main.c shared/lockfree_stack.c shared/lockfree_stack.h)

add_compile_definitions(FAIRLOCK)