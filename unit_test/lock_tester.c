#define _GNU_SOURCE

#include "lock_tester.h"

#include <ccsynch.h>
#include <common.h>
#include <flatcombining.h>
#include <rcl.h>

#include <assert.h>
#include <execinfo.h>
#include <sched.h>

#define ITERATION 50000
#define THREAD_COUNT 32
#define REPEAT_COUNT 64

typedef enum
{
	FLAT_COMBINING,
	CC_SYNCH,
	RCL
} LOCK_TYPE;

typedef struct
{
	int id;
	LOCK_TYPE type;
} task_t;

u_int64_t volatile global_counter = 0;

fc_lock_t fcLock;
cc_synch_t ccSynch;

rcl_lock_t rclLock;
rcl_server_t rclServer;

void* job(void* arg)
{
	//	task_t* task = arg;
	u_int32_t counter = 0;
	while(counter++ < ITERATION)
	{
		global_counter++;
	}

	return NULL;
}

void* worker(void* args)
{
	task_t* task = args;

	int counter = 0;
	if (task->type == RCL)
	{
		rcl_register_client(&rclServer);
	}

	while(counter++ < REPEAT_COUNT)
	{
		switch(task->type)
		{
		case FLAT_COMBINING:
			fc_lock(&fcLock, &job, args);
			break;
		case CC_SYNCH:
			cc_synch_lock(&ccSynch, &job, args);
			break;
		case RCL:
			rcl_lock(&rclLock, &job, args);
		}
	}

	return NULL;
}

void fc_cc_test(){
	fc_init(&fcLock);
	cc_synch_init(&ccSynch);

	task_t tasks[THREAD_COUNT];
	pthread_t pthreads[THREAD_COUNT];

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		tasks[i].id = i;
		tasks[i].type = FLAT_COMBINING;
		pthread_create(&pthreads[i], NULL, &worker, &tasks[i]);
	}

	for(int i = 0; i < sizeof(pthreads) / sizeof(pthreads[0]); ++i)
	{
		pthread_join(pthreads[i], NULL);
	}

	printf("Type: %s\n", "Flat Combining");
	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION * REPEAT_COUNT);
	printf("ACTUAL %lu\n\n", global_counter);

	global_counter = 0;

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		tasks[i].id = i;
		tasks[i].type = CC_SYNCH;
		pthread_create(&pthreads[i], NULL, &worker, &tasks[i]);
	}

	for(int i = 0; i < sizeof(pthreads) / sizeof(pthreads[0]); ++i)
	{
		pthread_join(pthreads[i], NULL);
	}

	printf("Type: %s\n", "CCSynch");
	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION * REPEAT_COUNT);
	printf("ACTUAL %lu\n\n", global_counter);
}

void rcl_test()
{
	global_counter = 0;

	int numberOfProcessors = sysconf(_SC_NPROCESSORS_ONLN);

	printf("Number of processors: %d\n", numberOfProcessors);

	rcl_server_init(&rclServer, numberOfProcessors - 1);

	rcl_lock_init(&rclLock, &rclServer);

	task_t tasks[THREAD_COUNT];
	pthread_t pthreads[THREAD_COUNT];

	pthread_attr_t attr;
	cpu_set_t cpus;
	pthread_attr_init(&attr);

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		CPU_ZERO(&cpus);
		CPU_SET(i % (numberOfProcessors - 1), &cpus);

		tasks[i].id = i;
		tasks[i].type = RCL;
		pthread_create(&pthreads[i], &attr, &worker, &tasks[i]);
	}

	for(int i = 0; i < sizeof(pthreads) / sizeof(pthreads[0]); ++i)
	{
		pthread_join(pthreads[i], NULL);
	}

	printf("Type: %s\n", "RCL");
	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION * REPEAT_COUNT);
	printf("ACTUAL %lu\n\n", global_counter);
}

void lock_test()
{
	fc_cc_test();
	rcl_test();
}