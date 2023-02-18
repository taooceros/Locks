#ifndef LOCK_FLAT_COMBINING_FAIR_H
#define LOCK_FLAT_COMBINING_FAIR_H

#include <common.h>
#include <pthread.h>
#include <sched.h>
#include <shared.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

typedef struct fcf_thread_node
{
	int age;
	bool active;
	func_ptr_t delegate;
	void* args;
	void* response;
	struct fcf_thread_node* next;
	pthread_t pthread;
	ull banned_until;
} fcf_thread_node;

typedef struct
{
	int pass;
	atomic_flag flag;
	_Atomic(fcf_thread_node*) head;
	pthread_key_t fcthread_info_key;
	atomic_int num_waiting_threads;
} fcf_lock_t;

void fcf_init(fcf_lock_t* lock);

void* fcf_lock(fcf_lock_t* lock, void* (*func_ptr)(void*), void* arg);

#endif /* LOCK_FLAT_COMBINING_FAIR_H */