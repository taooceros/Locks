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

static inline long futex(int* uaddr, int futex_op, int val, const struct timespec* timeout)
{
	return syscall(SYS_futex, uaddr, futex_op, val, timeout, NULL, 0);
}

void wait_on_futex_value(int* uaddr, int value)
{
	while(atomic_load(uaddr) != value)
	{
		long rc = futex(uaddr, FUTEX_WAIT, value, NULL);
		if(rc == -1)
		{
			perror("futex");
			exit(1);
		}
		else if(rc != 0)
		{
			abort();
		}
	}
}

void wake_futex_blocking(int* uaddr)
{
	while(1)
	{
		long rc = futex(uaddr, FUTEX_WAKE, 1, NULL);
		if(rc == -1)
		{
			perror("futex");
			exit(1);
		}
		else if(rc > 0)
		{
			return;
		}
	}
}

#endif //LOCKS_SHARED_H
