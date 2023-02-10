//
// Created by 1 on 2/6/2023.
//

#ifndef LOCKS_LOCKFREE_STACK_H
#define LOCKS_LOCKFREE_STACK_H

typedef struct lockfree_stack_t lockfree_stack_t;

void lockfree_stack_init(lockfree_stack_t* stack);

void lockfree_stack_push(lockfree_stack_t* stack, void* data);

void* lockfree_stack_pop(lockfree_stack_t* stack);

#endif //LOCKS_LOCKFREE_STACK_H
