//
// Created by 1 on 2/6/2023.
//

#include "rcl.h"
#include <sched.h>
#include <stdio.h>

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