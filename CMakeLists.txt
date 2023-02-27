cmake_minimum_required(VERSION 3.22)

project(locks)

set(CMAKE_C_FLAGS "-pthread -g -Wall -O2")
set(CMAKE_C_FLAGS_DEBUG "-pthread -g -Wall")
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY bin)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

add_compile_definitions(CYCLE_PER_US=2700)
add_compile_definitions(FC_THREAD_MAX_CYCLE=CYCLE_PER_MS*10)
add_compile_definitions(_GNU_SOURCE)

include_directories(CCsynch FlatCombining/original FlatCombining/fair_ban FlatCombining/fair_pq)
include_directories(shared)
include_directories(RCL)
include_directories(ticket)

FILE(GLOB_RECURSE FC FlatCombining/*.c)
FILE(GLOB_RECURSE CCSYNCH CCsynch/*.c)
FILE(GLOB_RECURSE RCL RCL/*.c)
FILE(GLOB_RECURSE SHARED shared/*.c)
FILE(GLOB TICKET ticket/*.c)


add_executable(example example.c
        ${FC}
        ${CCSYNCH}
        ${RCL}
        ${SHARED}
        ${TICKET})

add_executable(test
        shared/lockfree_stack.c
        unit_test/lockfree_stack_test.c
        "unit_test/unit_test.c"
        "unit_test/lock_tester.c"
        FlatCombining/original/flatcombining.c
        FlatCombining/fair_ban/flatcombiningfair.c
        FlatCombining/fair_pq/flatcombiningfairpq.c
        CCsynch/ccsynch.c RCL/rcl.c RCL/rcl.h)


add_executable(usclexample proportional-share/u-scl/example/main.c shared/lockfree_stack.c shared/lockfree_stack.h)

add_compile_definitions(FAIRLOCK)