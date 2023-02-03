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
	fc_thread_node* previous = NULL;

	bool isHead = true;

	while(current != NULL)
	{
		if(current->delegate != NULL)
		{
			current->age = lock->pass;
			current->response = current->delegate(current->args);
			current->delegate = NULL;
		}

		if(isHead)
		{
			isHead = false;
		}
		else
		{
			if(lock->pass - current->age > 50)
			{
				previous->next = current->next;
				current->active = false;
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
	}

	return node;
}

static void ensureNodeActive(fc_lock_t* lock, fc_thread_node* node)
{
	if(!node->active)
	{
		fc_thread_node** oldHead = &lock->head;
		node->next = *oldHead;
		while(!__atomic_compare_exchange(
			&lock->head, oldHead, &node, false, __ATOMIC_RELEASE, __ATOMIC_RELAXED))
		{
			oldHead = &lock->head;
			node->next = *oldHead;
		}
	}
}

void* fc_lock(fc_lock_t* lock, void* (*func_ptr)(void*), void* arg)
{
	fc_thread_node* node = retrieveNode(lock);
	node->delegate = func_ptr;
	node->args = arg;

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
		if(atomic_flag_test_and_set_explicit(&lock->flag, __ATOMIC_ACQUIRE))
		{
			goto spin_and_wait_or_retry;
		}
		else
		{
			// act as the combinator
			scanCombineApply(lock);
			atomic_flag_clear_explicit(&lock->flag, __ATOMIC_RELEASE);
		}
	}

	return node->response;

}
