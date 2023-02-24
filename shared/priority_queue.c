#include "priority_queue.h"
#include <stdlib.h>

#define var __auto_type
#define PARENT(i)                                                                                  \
	({                                                                                             \
		__auto_type _i = i;                                                                        \
		(_i - 1) / 2;                                                                              \
	})
#define LEFT(i)                                                                                    \
	({                                                                                             \
		__auto_type _i = i;                                                                        \
		_i * 2 + 1;                                                                                \
	})
#define RIGHT(i)                                                                                   \
	({                                                                                             \
		__auto_type _i = i;                                                                        \
		_i * 2 + 2;                                                                                \
	})

int pq_init(pq_t* pq)
{
	pq->data = (pq_node_t*)malloc(PQ_DEFAULT_CAPACITY * sizeof(pq_node_t));
	if(pq->data == NULL)
		return -1;
	pq->size = 0;
	pq->capacity = PQ_DEFAULT_CAPACITY;
	return 0;
}

void pq_destroy(pq_t* pq)
{
	free(pq->data);
}

void swim(pq_t* pq, int i)
{
	var data = pq->data;
	int parent;
	while(i > 0 && data[parent = PARENT(i)].priority < data[i].priority)
	{
		var temp = data[i];
		data[i] = data[parent];
		data[parent] = temp;
		i = parent;
	}
}

void sink(pq_t* pq, int i)
{
	var data = pq->data;
	int left, right, largest;
	while((LEFT(i) < pq->size))
	{
		left = LEFT(i);
		right = RIGHT(i);
		largest = i;
		if(data[left].priority > data[largest].priority)
			largest = left;
		if(right < pq->size && data[right].priority > data[largest].priority)
			largest = right;
		if(largest == i)
			break;
		var temp = data[i];
		data[i] = data[largest];
		data[largest] = temp;
		i = largest;
	}
}

/*
* Return 0 if successful, -1 if failed
*/
int pq_push(pq_t* pq, int priority, void* data)
{
	if(pq->size == pq->capacity)
	{
		pq->capacity *= 2;
		var old_data = pq->data;
		pq->data = (pq_node_t*)realloc(pq->data, pq->capacity * sizeof(pq_node_t));
		if(pq->data == NULL)
		{
			pq->data = old_data;
			return -1;
		}
	}

	int i = pq->size++;

	var node = &pq->data[i];

	node->priority = priority;
	node->data = data;

	swim(pq, i);

	return 0;
}

/*
* Return 0 if successful, -1 if empty
*/
int pq_pop(pq_t* pq, int* priority, void** data)
{
	if(pq->size == 0)
		return -1;

	var node = &pq->data[0];

	if(priority != NULL)
		*priority = node->priority;
	if(data != NULL)
		*data = node->data;

	pq->data[0] = pq->data[--pq->size];

	sink(pq, 0);

	return 0;
}

int pq_change_priority(pq_t* pq, int index, int new_priority)
{
	if(index < 0 || index >= pq->size)
		return -1;

	var node = &pq->data[index];

	int old_priority = node->priority;
	node->priority = new_priority;

	if(new_priority > old_priority)
		swim(pq, index);
	else
		sink(pq, index);

	return 0;
}

void* pq_peek(pq_t* pq)
{
	if(pq->size == 0)
		return NULL;
	return pq->data[0].data;
}