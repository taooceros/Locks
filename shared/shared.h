//
// Created by 1 on 2/6/2023.
//

#ifndef LOCKS_SHARED_H
#define LOCKS_SHARED_H
#include "lockfree_stack.h"
#include <linux/futex.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <unistd.h>

typedef void* (*func_ptr_t)(void*);

#endif //LOCKS_SHARED_H
