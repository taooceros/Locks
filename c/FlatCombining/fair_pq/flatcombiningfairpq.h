#ifndef LOCK_FLAT_COMBINING_FAIR_PQ_H
#define LOCK_FLAT_COMBINING_FAIR_PQ_H

#include <pqueue.h>
#include <pthread.h>
#include <sched.h>
#include <shared.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

#ifndef FC_THREAD_MAX_CYCLE
#	error "FC_THREAD_MAX_CYCLE not defined"
#endif

typedef struct fcfpq_thread_node
{
	int age;
	bool active;
	_Atomic(func_ptr_t) delegate;
	bool queued;
	void* args;
	void* response;
	struct fcfpq_thread_node* next;
	pthread_t pthread;
	ull usage;
	int pos;
} fcfpq_thread_node;

typedef struct
{
	int pass;
	atomic_bool flag;
	_Atomic(fcfpq_thread_node*) head;
	pthread_key_t fcfpqthread_info_key;
	// statistics
	long long num_exec;
	long long avg_cs;
	pqueue_t* thread_pq;
} fcfpq_lock_t;

void fcfpq_init(fcfpq_lock_t* lock);

void* fcfpq_lock(fcfpq_lock_t* lock, void* (*func_ptr)(void*), void* arg);

#endif /* LOCK_FLAT_COMBINING_FAIR_H */