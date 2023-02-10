//
// Created by 1 on 2/6/2023.
//

#include "lockfree_stack.h"
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
	new_node->next = stack->head;
	stack->head = new_node;
}

void* lockfree_stack_pop(lockfree_stack_t* stack) { }