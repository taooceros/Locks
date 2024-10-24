#ifndef LOCK_FLAT_COMBINING_H
#define LOCK_FLAT_COMBINING_H

#include <pthread.h>
#include <sched.h>
#include <shared.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

typedef struct fc_thread_node
{
	int age;
	bool active;
	_Atomic func_ptr_t delegate;
	void* args;
	void* response;
	struct fc_thread_node* next;
	pthread_t pthread;
} fc_thread_node;

typedef struct
{
	int pass;
	atomic_bool flag;
	_Atomic(fc_thread_node*) head;
	pthread_key_t fcthread_info_key;
} fc_lock_t;

void fc_init(fc_lock_t* lock);

void* fc_lock(fc_lock_t* lock, void* (*func_ptr)(void*), void* arg);

#endif /* LOCK_FLAT_COMBINING_H */