#include <common.h>
#include <immintrin.h>
#include <rdtsc.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>

#include "flatcombiningfairpq.h"
#include <pqueue.h>

#define var __auto_type

static int cmp_pri(pqueue_pri_t next, pqueue_pri_t curr)
{
	return (next < curr);
}

static pqueue_pri_t get_pri(void* a)
{
	return ((fcfpq_thread_node*)a)->usage;
}

static void set_pri(void* a, pqueue_pri_t pri)
{
	((fcfpq_thread_node*)a)->usage = pri;
}

static size_t get_pos(void* a)
{
	return ((fcfpq_thread_node*)a)->pos;
}

static void set_pos(void* a, size_t pos)
{
	((fcfpq_thread_node*)a)->pos = pos;
}

void fcfpq_init(fcfpq_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	lock->avg_cs = 0;
	lock->num_exec = 0;
	lock->thread_pq = pqueue_init(16, cmp_pri, get_pri, set_pri, get_pos, set_pos);
	pthread_key_create(&lock->fcfpqthread_info_key, NULL);
}

static void addNewRegisteredJob(fcfpq_lock_t* lock)
{
	var current = lock->head;

	while(current != NULL)
	{
		if(current->queued == false && current->delegate != NULL)
		{
			pqueue_insert(lock->thread_pq, current);
		}

		current = current->next;
	}
}

static void scanCombineApply(fcfpq_lock_t* lock)
{
	lock->pass++;

	fcfpq_thread_node* current;
	int usage;

	if(pqueue_peek(lock->thread_pq) == NULL)
	{
		return;
	}

	ull begin = rdtscp();
	ull now;

	while(((now = rdtscp()) - begin) < FC_THREAD_MAX_CYCLE &&
		  (current = pqueue_pop(lock->thread_pq)) != NULL)
	{
		current->queued = false;

		if(current->delegate != NULL)
		{
			ull begin = rdtscp();
			current->age = lock->pass;

			lock->num_exec++;
			current->response = current->delegate(current->args);
			current->delegate = NULL;
			ull end = rdtscp();

			usage -= end - begin;
			current->usage = usage;
		}
	}

	static int counter = 0;

	// printf("finish scan once %d\n", counter++);
}

static inline void tryCleanUp(fcfpq_lock_t* lock)
{
	return;

	if(lock->pass % 50)
		return;

	fcfpq_thread_node* previous = NULL;
	fcfpq_thread_node* current = lock->head;

	while(current != NULL)
	{
		if(previous != NULL)
		{
			if(lock->pass - current->age > 50)
			{
				current->active = false;
				previous->next = current->next;
				current = current->next;
				// printf("remove node \n");
				if(current == NULL)
					return;
			}
		}

		previous = current;
		current = current->next;
	}
}

static fcfpq_thread_node* retrieveNode(fcfpq_lock_t* lock)
{
	fcfpq_thread_node* node = (fcfpq_thread_node*)pthread_getspecific(lock->fcfpqthread_info_key);

	if(node == NULL)
	{
		node = (fcfpq_thread_node*)malloc(sizeof(fcfpq_thread_node));
		node->active = false;
		node->age = 0;
		node->pthread = pthread_self();
		node->usage = 0;
		node->delegate = NULL;
		node->args = NULL;
		node->response = NULL;
		node->queued = false;
		pthread_setspecific(lock->fcfpqthread_info_key, node);
	}

	return node;
}

static void ensureNodeActive(fcfpq_lock_t* lock, fcfpq_thread_node* node)
{
	if(!node->active)
	{
		fcfpq_thread_node* oldHead;
		do
		{
			oldHead = lock->head;
			node->next = oldHead;
		} while(!atomic_compare_exchange_weak(&(lock->head), &oldHead, node));

		node->active = true;
	}
}

void* fcfpq_lock(fcfpq_lock_t* lock, void* (*func_ptr)(void*), void* arg)
{
	fcfpq_thread_node* node = retrieveNode(lock);
	node->delegate = func_ptr;
	node->args = arg;
	node->response = NULL;

	int counter = 0;

acquire_lock_or_spin:
	ensureNodeActive(lock, node);
	// lock has been taken

	if(lock->flag && node->delegate != NULL)
	{
	spin_and_wait_or_retry:
		while(node->delegate != NULL)
		{
			if(++counter > SPIN_LIMIT)
			{
				counter = 0;
				sched_yield();
				goto acquire_lock_or_spin;
			}
			_mm_pause();
		}
	}
	// try to become the combinator
	else
	{
		if(atomic_exchange(&lock->flag, true))
		{
			goto spin_and_wait_or_retry;
		}
		else
		{
			// act as the combinator
			addNewRegisteredJob(lock);
			scanCombineApply(lock);
			tryCleanUp(lock);
			atomic_store(&lock->flag, false);

			if(node->delegate != NULL)
				goto acquire_lock_or_spin;
			// TODO: deadlock when try to become the combiner again???
		}
	}

	return node->response;
}
