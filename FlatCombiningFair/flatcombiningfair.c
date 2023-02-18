#include "flatcombiningfair.h"
#include <emmintrin.h>
#include <rdtsc.h>
#include <stdio.h>

void fcf_init(fcf_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	lock->num_waiting_threads = 0;
	pthread_key_create(&lock->fcthread_info_key, NULL);
}

static void scanCombineApply(fcf_lock_t* lock)
{
	lock->pass++;

	fcf_thread_node* current = lock->head;

	while(current != NULL)
	{
		if(current->delegate != NULL)
		{
			ull begin = rdtscp();
			if(current->banned_until > begin)
			{
				// printf("should wait %lld\n", (current->banned_until - begin) / CYCLE_PER_MS);

				goto scan_continue;
			}

			current->age = lock->pass;
			current->response = current->delegate(current->args);
			current->delegate = NULL;
			ull end = rdtscp();
			// TODO: why this doesn't work??????????????????????????/
			// lock->num_waiting_threads--;
			current->banned_until = begin + (end - begin) * lock->num_waiting_threads;
		}

	scan_continue:
		current = current->next;
	}
}

static inline void tryCleanUp(fcf_lock_t* lock)
{
	if(lock->pass % 50)
		return;

	fcf_thread_node* previous = NULL;
	fcf_thread_node* current = lock->head;

	while(current != NULL)
	{
		if(previous != NULL)
		{
			if(lock->pass - current->age > 50)
			{
				current->active = false;
				previous->next = current->next;
			}
		}

		previous = current;
		current = current->next;
	}
}

static fcf_thread_node* retrieveNode(fcf_lock_t* lock)
{
	fcf_thread_node* node = (fcf_thread_node*)pthread_getspecific(lock->fcthread_info_key);

	if(node == NULL)
	{
		node = (fcf_thread_node*)malloc(sizeof(fcf_thread_node));
		node->active = false;
		node->age = 0;
		node->pthread = pthread_self();
		node->banned_until = 0;
		pthread_setspecific(lock->fcthread_info_key, node);
	}

	return node;
}

static void ensureNodeActive(fcf_lock_t* lock, fcf_thread_node* node)
{
	if(!node->active)
	{
		fcf_thread_node* oldHead;
		do
		{
			oldHead = lock->head;
			node->next = oldHead;
		} while(!atomic_compare_exchange_weak(&(lock->head), &oldHead, node));

		node->active = true;
	}
}

void* fcf_lock(fcf_lock_t* lock, void* (*func_ptr)(void*), void* arg)
{
	fcf_thread_node* node = retrieveNode(lock);
	node->delegate = func_ptr;
	node->args = arg;
	node->response = NULL;

	ensureNodeActive(lock, node);
	// lock has been taken
	int counter = 0;
	lock->num_waiting_threads++;

	while(atomic_flag_test_and_set(&lock->flag))
	{
		while(node->delegate != NULL)
		{
			if(++counter > 100)
			{
				counter = 0;
				sched_yield();
				break;
			}
			_mm_pause();
		}

		if(node->delegate == NULL)
			break;
	}

	if(node->delegate != NULL)
	{ // act as the combinator
		scanCombineApply(lock);
		tryCleanUp(lock);
		atomic_flag_clear(&lock->flag);
	}

	// TODO: why????
	lock->num_waiting_threads--;

	return node->response;
}
