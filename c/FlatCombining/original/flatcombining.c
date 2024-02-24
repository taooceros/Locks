#include "flatcombining.h"
#include <x86intrin.h>
#include <stdatomic.h>

void fc_init(fc_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	pthread_key_create(&lock->fcthread_info_key, NULL);
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

static fc_thread_node* retrieveNode(fc_lock_t* lock)
{
	fc_thread_node* node = (fc_thread_node*)pthread_getspecific(lock->fcthread_info_key);

	if(node == NULL)
	{
		node = (fc_thread_node*)malloc(sizeof(fc_thread_node));
		node->active = false;
		node->age = 0;
		node->pthread = pthread_self();
		pthread_setspecific(lock->fcthread_info_key, node);
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
	node->args = arg;
	node->response = NULL;
	node->delegate = func_ptr;

	int counter = 0;

acquire_lock_or_spin:
	ensureNodeActive(lock, node);
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
		if(atomic_exchange(&lock->flag, 1))
		{
			goto spin_and_wait_or_retry;
		}
		else
		{
			// act as the combinator
			scanCombineApply(lock);
			tryCleanUp(lock);
			atomic_store(&lock->flag, false);
		}
	}

	atomic_thread_fence(memory_order_release);

	return node->response;
}
