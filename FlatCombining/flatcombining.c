#include "flatcombining.h"
#include <emmintrin.h>

static void free_key(void* key)
{
	free(key);
}

void fc_init(fc_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	pthread_key_create(&lock->fcthread_info_key, &free_key);
}

static void scanCombineApply(fc_lock_t* lock)
{
	lock->pass++;

	fc_thread_node* current = lock->head;

	while(current != NULL)
	{
		if(current->delegate != NULL)
		{
			current->age = lock->pass;
			current->response = current->delegate(current->args);
			current->delegate = NULL;
		}
		current = current->next;
	}
}

static inline void tryCleanUp(fc_lock_t* lock)
{
	if(lock->pass % 50)
		return;

	fc_thread_node* previous = NULL;
	fc_thread_node* current = lock->head;

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

static fc_thread_node* retrieveNode(fc_lock_t* lock)
{
	static __thread fc_thread_node* node = NULL;

	if(node == NULL)
	{
		node = (fc_thread_node*)malloc(sizeof(fc_thread_node));
		node->active = false;
		node->age = 0;
		node->pthread = pthread_self();
	}

	return node;
}

static void ensureNodeActive(fc_lock_t* lock, fc_thread_node* node)
{
	if(!node->active)
	{
		fc_thread_node* oldHead;
		do
		{
			oldHead = lock->head;
			node->next = oldHead;
		} while(!atomic_compare_exchange_weak(&(lock->head), &oldHead, node));

		node->active = true;
	}
}

void* fc_lock(fc_lock_t* lock, void* (*func_ptr)(void*), void* arg)
{
	fc_thread_node* node = retrieveNode(lock);
	node->delegate = func_ptr;
	node->args = arg;
	node->response = NULL;

	ensureNodeActive(lock, node);
	// lock has been taken
	int counter = 0;

acquire_lock_or_spin:
	if(lock->flag)
	{
	spin_and_wait_or_retry:
		while(node->delegate != NULL)
		{
			if(++counter > 100)
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
		if(atomic_flag_test_and_set(&lock->flag))
		{
			goto spin_and_wait_or_retry;
		}
		else
		{
			// act as the combinator
			scanCombineApply(lock);
			tryCleanUp(lock);
			atomic_flag_clear(&lock->flag);
		}
	}

	return node->response;
}
