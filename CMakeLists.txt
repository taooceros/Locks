cmake_minimum_required(VERSION 3.22)

project(locks)

set(CMAKE_C_FLAGS -pthread)

add_executable(example example.c
        FlatCombining/flatcombining.c)