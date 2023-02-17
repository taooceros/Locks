//
// Created by 1 on 2/6/2023.
//

#include "lockfree_stack_test.h"
#include <lockfree_stack.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

#define NUM_THREADS 128
#define NUM_DATA 100000
#define TOTAL_NUM_DATA (NUM_DATA * NUM_THREADS)

atomic_int_fast32_t global_index = 0;
int global_data[TOTAL_NUM_DATA];
bool success[TOTAL_NUM_DATA] = {false};

static void add_data(lockfree_stack_t* stack)
{
	for(int i = 0; i < NUM_DATA; ++i)
	{
		lockfree_stack_push(stack, &global_data[global_index++]);
	}
}

static void pop_data(lockfree_stack_t* stack)
{
	for(int i = 0; i < NUM_DATA; ++i)
	{
		int* data = (int*)lockfree_stack_pop(stack);
		success[*data] = true;
	}
}

void lockfree_stack_test()
{
	lockfree_stack_t stack;
	lockfree_stack_init(&stack);

	for(int i = 0; i < TOTAL_NUM_DATA; i++)
	{
		global_data[i] = i;
	}

	pthread_t threads[NUM_THREADS];

	for(int i = 0; i < NUM_THREADS; ++i)
	{
		pthread_create(&threads[i], NULL, (void*)add_data, &stack);
	}

	for(int i = 0; i < NUM_THREADS; ++i)
	{
		pthread_join(threads[i], NULL);
	}

	for(int i = 0; i < TOTAL_NUM_DATA; ++i)
	{
		int* data = (int*)lockfree_stack_pop(&stack);
		success[*data] = true;
	}

	for(int i = 0; i < TOTAL_NUM_DATA; ++i)
	{
		if(!success[i])
		{
			printf("Failed to pop data %d\n", i);
		}
	}

	// inner_test_lock pop concurrently

	// insert data

	for(int i = 0; i < TOTAL_NUM_DATA; ++i)
	{
		lockfree_stack_push(&stack, &global_data[i]);
	}

	// pop data concurrently

	for(int i = 0; i < NUM_THREADS; ++i)
	{
		pthread_create(&threads[i], NULL, (void*)pop_data, &stack);
	}

	for(int i = 0; i < NUM_THREADS; ++i)
	{
		pthread_join(threads[i], NULL);
	}

	for(int i = 0; i < TOTAL_NUM_DATA; ++i)
	{
		if(!success[i])
		{
			printf("Failed to pop data %d\n", i);
		}
	}
}