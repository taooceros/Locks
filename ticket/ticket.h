#ifndef TICKET_H
#define TICKET_H

#include <stddef.h>
#include <stdint.h>

typedef struct ticket_lock
{
	volatile uint64_t next_ticket;
	volatile uint64_t now_serving;
} ticket_lock_t;

void ticket_init(ticket_lock_t* lock);

void ticket_lock(ticket_lock_t* lock);

void ticket_unlock(ticket_lock_t* lock);

#endif