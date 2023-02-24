#ifndef LOCK_FLAT_COMBINING_FAIR_PQ_H
#define LOCK_FLAT_COMBINING_FAIR_PQ_H

#include "priority_queue.h"
#include <pthread.h>
#include <sched.h>
#include <shared.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

#ifndef FC_THREAD_MAX_NS
#error "FC_THREAD_MAX not defined"
#endif

typedef struct fcfpq_thread_node
{
	int age;
	bool active;
	func_ptr_t delegate;
	void* args;
	void* response;
	struct fcfpq_thread_node* next;
	pthread_t pthread;
	ull usage;
} fcfpq_thread_node;

typedef struct
{
	int pass;
	bool flag;
	fcfpq_thread_node* head;
	pthread_key_t fcfpqthread_info_key;
	atomic_int num_waiting_threads;
	// statistics
	long long num_exec;
	long long avg_cs;
	pq_t thread_pq;
} fcfpq_lock_t;

void fcfpq_init(fcfpq_lock_t* lock);

void* fcfpq_lock(fcfpq_lock_t* lock, void* (*func_ptr)(void*), void* arg);

#endif /* LOCK_FLAT_COMBINING_FAIR_H */