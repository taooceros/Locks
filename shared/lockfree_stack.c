//
// Created by 1 on 2/6/2023.
//

#include "lockfree_stack.h"
#include <stdatomic.h>
#include <stddef.h>
#include <stdlib.h>


void lockfree_stack_init(lockfree_stack_t* stack)
{
	stack->head = NULL;
}

void lockfree_stack_push(lockfree_stack_t* stack, void* data)
{
	lockfree_stack_node_t* new_node = (lockfree_stack_node_t*)malloc(sizeof(lockfree_stack_node_t));
	new_node->data = data;
	lockfree_stack_node_t* oldHead;

	do
	{
		oldHead = stack->head;
		new_node->next = oldHead;
	} while(!atomic_compare_exchange_strong(&stack->head, &oldHead, new_node));
}

void* lockfree_stack_pop(lockfree_stack_t* stack)
{

	lockfree_stack_node_t* head;

	lockfree_stack_node_t* newHead;

	do
	{
		head = stack->head;
		if(head == NULL)
			return NULL;
		newHead = head->next;
	} while(!atomic_compare_exchange_strong(&stack->head, &head, newHead));

	void* data = head->data;
	free(head);
	return data;
}