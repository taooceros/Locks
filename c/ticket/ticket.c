// ticket lock

#include "ticket.h"
#include <x86intrin.h>
#include <stdatomic.h>

void ticket_init(ticket_lock_t* lock)
{
	lock->now_serving = 0;
	lock->next_ticket = 0;
}

void ticket_lock(ticket_lock_t* lock)
{
	atomic_uint_fast32_t ticket = atomic_fetch_add(&lock->next_ticket, 1);
	while(lock->now_serving != ticket)
		_mm_pause();
}

void ticket_unlock(ticket_lock_t* lock)
{
    atomic_fetch_add(&lock->now_serving, 1);
}