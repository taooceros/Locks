//
// Created by 1 on 2/6/2023.
//

#include "rcl.h"
#include <pthread.h>
#include <sched.h>
#include <stdio.h>

static inline long futex(int* uaddr, int futex_op, int val, const struct timespec* timeout)
{
	return syscall(SYS_futex, uaddr, futex_op, val, timeout, NULL, 0);
}

static inline void wait_on_futex_value(int* uaddr, int value)
{
	while(atomic_load(uaddr) != value)
	{
		long rc = futex(uaddr, FUTEX_WAIT, value, NULL);
		if(rc == -1)
		{
			perror("futex");
			exit(1);
		}
		else if(rc != 0)
		{
			abort();
		}
	}
}

static inline void wake_futex_blocking(int* uaddr)
{
	while(1)
	{
		long rc = futex(uaddr, FUTEX_WAKE, 1, NULL);
		if(rc == -1)
		{
			perror("futex");
			exit(1);
		}
		else if(rc > 0)
		{
			return;
		}
	}
}

int number_of_clients = 0;

void rcl_serving_thread(rcl_thread_t* t)
{
	rcl_server_t* s = t->server;

	while(true)
	{
		s->is_alive = true;
		t->timestamp = s->timestamp;
		s->num_free_threads--;

		for(int i = 0; i < number_of_clients; ++i)
		{
			rcl_request_t* r = &s->requests[i];
			if(r->delegate != NULL)
			{
				rcl_lock_t* l = r->lock;

				int not_holding = 0;
				if(!atomic_compare_exchange_weak(&l->holder, &not_holding, i))
				{
					func_ptr_t delegate = r->delegate;
					if(delegate != NULL)
					{
						r->args = delegate(r->args);
						r->delegate = NULL;
					}
					l->holder = 0;
				}
			}
		}

		s->num_free_threads++;

		if(s->num_serving_threads > 1)
		{
			if(s->num_free_threads <= 1)
			{
				sched_yield();
			}
			else
			{
				t->is_servicing = false;
				s->num_serving_threads--;
				lockfree_stack_push(s->prepared_threads, t);
				wait_on_futex_value(&t->is_servicing, true);
			}
		}
	}
}

rcl_thread_t* allocate_serving_threads(rcl_server_t* s) { }

void rcl_start_serving_thread(rcl_thread_t* t)
{
	pthread_create(&t->pthread, NULL, (void* (*)(void*))rcl_serving_thread, t);
}

_Noreturn void backup_thread(rcl_server_t* server)
{
	while(true)
	{
		server->is_alive = false;
		wake_futex_blocking(&server->management_alive);
	}
}

_Noreturn void rcl_management_thread(rcl_server_t* s)
{
	rcl_thread_t* thread;

	s->is_alive = false;
	s->timestamp = 1;

	while(true)
	{
		if(!s->is_alive)
		{
			s->is_alive = true;

			if(s->num_free_threads == 0)
			{
				s->num_serving_threads++;
				s->num_free_threads++;

				thread = lockfree_stack_pop(s->prepared_threads);

				if(thread == NULL)
				{
					thread = allocate_serving_threads(s);
					rcl_thread_node_t* threadNode = malloc(sizeof(rcl_thread_node_t));
					threadNode->thread = thread;
					threadNode->next = s->threads;
					s->threads = threadNode;
					thread->is_servicing = true;
					rcl_start_serving_thread(thread);
				}
				else
				{
					thread->is_servicing = true;
					wake_futex_blocking(&thread->is_servicing);
				}
			}

			while(true)
			{
				rcl_thread_node_t* threadNode = s->threads;

				while(threadNode != NULL)
				{
					thread = threadNode->thread;
					if(thread->is_servicing && thread->timestamp < s->timestamp)
					{
						thread->timestamp = s->timestamp;
						wake_futex_blocking(&thread->is_servicing);
						goto end;
					}
				}
			}
		}
		else
		{
			s->is_alive = false;
		}
	end:
		wait_on_futex_value(&s->management_alive, true);
	}
}

void rcl_server_init(rcl_server_t* s, int cpu)
{
	s->num_serving_threads = 0;
	s->num_free_threads = 0;
	lockfree_stack_init(s->prepared_threads);
	s->threads = NULL;
	s->timestamp = 0;
	s->management_alive = false;
	s->is_alive = false;
	s->cpu = cpu;
	pthread_create(&s->management_thread, NULL, (void* (*)(void*))rcl_management_thread, s);
}

__thread int client_index;
__thread bool is_server_thread;
__thread rcl_server_t *server;

void rcl_lock(rcl_lock_t* l, func_ptr_t delegate, void* context)
{
	int read_me;
	rcl_request_t* request;

	request = &l->server->requests[client_index];

}