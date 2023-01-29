cmake_minimum_required(VERSION 3.22)

project(locks)

add_executable(example example.c
    FlatCombining/flatcombining.c)