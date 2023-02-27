#include <stdatomic.h>
#include <stdbool.h>
#define SPIN_LIMIT 50

#include "flatcombiningfair.h"
#include <common.h>
#include <immintrin.h>
#include <rdtsc.h>
#include <stdio.h>

void fcf_init(fcf_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	lock->num_waiting_threads = 0;
	lock->avg_cs = 0;
	lock->num_exec = 0;
	pthread_key_create(&lock->fcfthread_info_key, NULL);
}

static inline void tryCleanUp(int pass, fcf_thread_node* start)
{
	if(start == NULL)
		return;

	fcf_thread_node* previous = start;
	fcf_thread_node* current = start->next;

	while(current != NULL)
	{
		if(pass - current->age > 50 && atomic_load(&current->delegate) != NULL)
		{
			current->active = false;
			previous->next = current->next;
			current = current->next;
			if(start->next == NULL)
			{
				printf("weird\n");
			}
			if(current == NULL)
				return;
		}

		previous = current;
		current = current->next;
	}
}

static void scanCombineApply(fcf_lock_t* lock)
{
	if(lock->head->next == NULL)
	{
		printf("damn\n");
	}

	lock->pass++;

	fcf_thread_node* curHead = lock->head;

	fcf_thread_node* current = curHead;

	while(current != NULL)
	{
		if(lock->pass - current->age > 50)
		{
			//			printf("%d too old node %d %lu\n",
			//				   current->delegate != NULL,
			//				   lock->pass - current->age,
			//				   current->pthread);
		}

		if(atomic_load_explicit(&current->delegate, memory_order_acquire) != NULL)
		{
			ull begin = rdtscp();
			current->age = lock->pass;
			if(current->banned_until > begin)
			{
				// printf("%ld should wait %lld\n",
				// 	   current->pthread,
				// 	   (current->banned_until - begin) / CYCLE_PER_MS);
				goto scan_continue;
			}

			lock->num_exec++;
			current->response = current->delegate(current->args);
			atomic_store(&current->delegate, NULL);
			ull end = rdtscp();
			// TODO: why this doesn't work??????????????????????????/
			// lock->num_waiting_threads--;

			long long cs = end - begin;
			lock->avg_cs = lock->avg_cs + (cs - lock->avg_cs) / lock->num_exec;
			// printf("average cs %lld\n", lock->avg_cs);
			current->banned_until = end + (cs) * (lock->num_waiting_threads) - lock->avg_cs;
		}

	scan_continue:
		current = current->next;
	}

	tryCleanUp(lock->pass, curHead);

	static int counter = 0;

	// printf("finish scan once %d\n", counter++);
}

static fcf_thread_node* retrieveNode(fcf_lock_t* lock)
{
	fcf_thread_node* node = (fcf_thread_node*)pthread_getspecific(lock->fcfthread_info_key);

	if(node == NULL)
	{
		node = (fcf_thread_node*)malloc(sizeof(fcf_thread_node));
		node->active = false;
		node->age = 0;
		node->pthread = gettid();
		node->banned_until = 0;
		pthread_setspecific(lock->fcfthread_info_key, node);
	}

	return node;
}

static void ensureNodeActive(fcf_lock_t* lock, fcf_thread_node* node)
{
	if(!atomic_load(&node->active))
	{
		fcf_thread_node* oldHead;
		do
		{
			oldHead = lock->head;
			node->next = oldHead;
		} while(!atomic_compare_exchange_weak(&(lock->head), &oldHead, node));

		//		printf("add node %lu\n", node->pthread);
		atomic_store_explicit(&node->active, true, memory_order_release);
	}
}

void* fcf_lock(fcf_lock_t* lock, void* (*func_ptr)(void*), void* arg)
{
	fcf_thread_node* node = retrieveNode(lock);
	node->args = arg;
	node->response = NULL;
	atomic_store_explicit(&node->delegate, func_ptr, memory_order_release);

	ensureNodeActive(lock, node);
	// lock has been taken
	int counter = 0;

	lock->num_waiting_threads++;

acquire_lock_or_spin:
	if(lock->flag)
	{
	spin_and_wait_or_retry:
		while(atomic_load(&node->delegate) != NULL)
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
			scanCombineApply(lock);
			atomic_store(&lock->flag, false);

			if(node->delegate != NULL)
				goto acquire_lock_or_spin;
			// TODO: deadlock when try to become the combiner again???
		}
	}

	// TODO: why????
	lock->num_waiting_threads--;

	return node->response;
}
