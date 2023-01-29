#include "flatcombining.h"

void fc_init(fc_lock_t* lock)
{
	lock->pass = 0;
	lock->head = NULL;
	pthread_key_create(&lock->fcthread_info_key, NULL);
}

static void scanCombineApply(fc_lock_t* lock)
{
	lock->pass++;

	thread_node_t* current = lock->head;
	thread_node_t* previous = NULL;

	bool isHead = true;
	bool cleanup = !lock->pass % 50;

	while(current != NULL)
	{
		if(current->delegate != NULL)
		{
			current->age = lock->pass;
			current->response = current->delegate(current->arg1, current->arg2);
		}

		if(isHead)
		{
			previous = current;
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

static thread_node_t* retriveNode(fc_lock_t* lock)
{
	thread_node_t* node = (thread_node_t*)pthread_getspecific(lock->fcthread_info_key);

	if(node == NULL)
	{
		node = (thread_node_t*)malloc(sizeof(thread_node_t));
		node->active = false;
		node->age = 0;
	}

	return node;
}

static void ensureNodeActive(fc_lock_t* lock, thread_node_t* node)
{
	if(!node->active)
	{
		thread_node_t** oldHead = &lock->head;
		node->next = *oldHead;
		while(!__atomic_compare_exchange(
			&lock->head, oldHead, &node, false, __ATOMIC_RELEASE, __ATOMIC_RELAXED))
		{
			oldHead = &lock->head;
			node->next = *oldHead;
		}
	}
}

static int wait_response(thread_node_t* node)
{
	int counter = 0;
	while(node->delegate != NULL)
	{
		if(++counter > 100)
		{
			counter = 0;
			sched_yield();
		}
	}

	return node->response;
}

int fc_lock(fc_lock_t* lock, int (*func_ptr)(int, int), int arg1, int arg2)
{
	thread_node_t* node = retriveNode(lock);
	node->delegate = func_ptr;
	node->arg1 = arg1;
	node->arg2 = arg2;

	ensureNodeActive(lock, node);
	// lock has been taken
	if(lock->flag)
	{
		return wait_response(node);
	}
	// try become the combinator
	else
	{
		if(__atomic_exchange_n(&lock->flag, 1, __ATOMIC_RELAXED))
		{
			// fail to become the combinator
			return wait_response(node);
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
