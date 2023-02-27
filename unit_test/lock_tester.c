#include "flatcombiningfairpq.h"
#include <sched.h>

#include "lock_tester.h"

#include <ccsynch.h>
#include <common.h>
#include <flatcombining.h>
#include <flatcombiningfair.h>
#include <rcl.h>

#include <assert.h>
#include <execinfo.h>

#define ITERATION 500
#define THREAD_COUNT 512
#define REPEAT_COUNT 300

#include "locktypeenum.h"

#define GENERATE_ENUM_STRINGS
#include "locktypeenum.h"

#undef GENERATE_ENUM_STRINGS

typedef struct
{
	int id;
	LOCK_TYPE type;
} task_t;

int64_t volatile global_counter = 0;

fc_lock_t fcLock;
fcf_lock_t fcfLock;
fcfpq_lock_t fcfpqLock;
cc_synch_t ccSynch;

rcl_lock_t coutner_lock_rcl;
rcl_server_t rcl_server;

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
	if(task->type == RCL)
	{
		rcl_register_client(&rcl_server);
	}

	while(counter++ < REPEAT_COUNT)
	{
		switch(task->type)
		{
		case FLAT_COMBINING:
			fc_lock(&fcLock, &job, args);
			break;
		case FLAT_COMBINING_FAIR:
			fcf_lock(&fcfLock, &job, args);
			break;
		case FLAT_COMBINING_FAIR_PQ:
			fcf_lock(&fcfLock, &job, args);
			break;
		case CC_SYNCH:
			cc_synch_lock(&ccSynch, &job, args);
			break;
		case RCL:
			rcl_lock(&coutner_lock_rcl, &job, args);
			break;
		}
	}

	return NULL;
}

void inner_test_lock(const LOCK_TYPE lock_type)
{
	task_t tasks[THREAD_COUNT];
	pthread_t pthreads[THREAD_COUNT];

	global_counter = 0;

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		tasks[i].id = i;
		tasks[i].type = lock_type;
		pthread_create(&pthreads[i], NULL, &worker, &tasks[i]);
	}

	for(int i = 0; i < sizeof(pthreads) / sizeof(pthreads[0]); ++i)
	{
		pthread_join(pthreads[i], NULL);
	}

	printf("Type: %s\n", GetStringLOCK_TYPE(lock_type));
	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION * REPEAT_COUNT);
	printf("ACTUAL %lu\n\n", global_counter);
}

void fc_cc_test()
{
	fc_init(&fcLock);
	fcf_init(&fcfLock);
	fcfpq_init(&fcfpqLock);
	cc_synch_init(&ccSynch);

	inner_test_lock(CC_SYNCH);

	inner_test_lock(FLAT_COMBINING);
	inner_test_lock(FLAT_COMBINING_FAIR);
	// inner_test_lock(FLAT_COMBINING_FAIR_PQ);
}

void rcl_test()
{
	global_counter = 0;

	int numberOfProcessors = sysconf(_SC_NPROCESSORS_ONLN);

	printf("Number of processors: %d\n", numberOfProcessors);

	rcl_server_init(&rcl_server, numberOfProcessors - 1);

	rcl_lock_init(&coutner_lock_rcl, &rcl_server);

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
	rcl_test();

	fc_cc_test();
}