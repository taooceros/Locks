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

outer:
	if(lock->flag)
	{
	spinwait:
		while(node->delegate != NULL)
		{
			if(++counter > 100)
			{
				counter = 0;
				sched_yield();
				goto outer;
			}
			_mm_pause();
		}
	}
	// try to become the combinator
	else
	{
		if(__atomic_exchange_n(&lock->flag, 1, __ATOMIC_RELAXED))
		{
			goto spinwait;
		}
		else
		{
			// act as the combinator
			scanCombineApply(lock);
			lock->flag = 0;
			return node->response;
		}
	}
}
