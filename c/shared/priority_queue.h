// header guard of priority_queue.h
#ifndef PRIORITY_QUEUE_H
#define PRIORITY_QUEUE_H

#define PQ_DEFAULT_CAPACITY 16

typedef struct __pq_node
{
	void* data;
	long long priority;
} pq_node_t;

typedef struct __pq
{
	pq_node_t* data;
	int size;
	int capacity;
} pq_t;

int pq_init(pq_t* pq);
int pq_push(pq_t* pq, int priority, void* data);
int pq_pop(pq_t* pq, int* priority, void** data);
int pq_peek(pq_t* pq, int* priority, void** data);
int pq_change_priority(pq_t* pq, int index, int new_priority);

#endif