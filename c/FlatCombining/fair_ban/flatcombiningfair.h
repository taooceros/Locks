#ifndef LOCK_FLAT_COMBINING_FAIR_BAN_H
#define LOCK_FLAT_COMBINING_FAIR_BAN_H

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
	atomic_bool active;
	_Atomic(func_ptr_t) delegate;
	void* args;
	void* response;
	struct fcf_thread_node* next;
	pthread_t pthread;
	ull banned_until;
} fcf_thread_node;

typedef struct
{
	int pass;
	atomic_bool flag;
	_Atomic(fcf_thread_node*) head;
	pthread_key_t fcfthread_info_key;
	atomic_int num_waiting_threads;
	// statistics
	long long num_exec;
	long long avg_cs;
} fcf_lock_t;

void fcf_init(fcf_lock_t* lock);

void* fcf_lock(fcf_lock_t* lock, void* (*func_ptr)(void*), void* arg);

#endif /* LOCK_FLAT_COMBINING_FAIR_H */