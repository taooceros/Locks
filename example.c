#include "FlatCombining/flatcombining.h"
#include <pthread.h>
#include <stdio.h>
volatile int global_counter = 0;

fc_lock_t counter_lock;

int increase_counter(int arg1, int arg2)
{
	int counter = 0;
	while(counter < 10000)
	{
		global_counter++;
		counter++;
	}
}

void* worker()
{
	fc_lock(&counter_lock, &increase_counter, 0, 0);

	return NULL;
}

int main()
{
	fc_init(&counter_lock);

	const int thread_count = 10;

	pthread_t threads[thread_count];

	for(int i = 0; i < thread_count; i++)
	{
		pthread_create(&threads[i], NULL, &worker, NULL);
	}

	for(int i = 0; i < thread_count; i++)
	{
		pthread_join(threads[i], NULL);
	}

	printf("Global Result %d\n", global_counter);
}