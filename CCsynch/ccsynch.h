//
// Created by 1 on 2/1/2023.
//

#ifndef LOCKS_CCSYNCH_H
#define LOCKS_CCSYNCH_H

#include <emmintrin.h>
#include <pthread.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>

#define atomic_exchange(ptr, val, ret, memorder) __atomic_exchange(ptr, val, ret, memorder)

typedef struct cc_request
{
	void* (*delegate)(void*);
	void* args;
} cc_request;

typedef struct cc_thread_node
{
	cc_request request;
	void* result;
	volatile bool wait;
	volatile bool completed;
	struct cc_thread_node* next;
} node_t;

typedef struct cc_synch
{
	node_t* Tail;
	pthread_key_t ccthread_info_key;
} cc_synch_t;

void cc_synch_init(cc_synch_t* cc);

void* cc_synch_lock(cc_synch_t* lock, void* delegate, void* args);

#endif //LOCKS_CCSYNCH_H
