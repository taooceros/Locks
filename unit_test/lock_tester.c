#include "lock_tester.h"

#include <ccsynch.h>
#include <common.h>
#include <flatcombining.h>

#include <assert.h>
#include <execinfo.h>

#define ITERATION 500000
#define THREAD_COUNT 32

typedef enum
{
	FLAT_COMBINING,
	CC_SYNCH
} LOCK_TYPE;

typedef struct
{
	int id;
	LOCK_TYPE type;
} task_t;

u_int64_t volatile global_counter = 0;

fc_lock_t fcLock;
cc_synch_t ccSynch;

void* job(void* arg)
{
	task_t* task = arg;
	u_int32_t counter = 0;
	while(counter++ < ITERATION)
	{
		global_counter++;
	}

	return NULL;
}

void* worker(void* args)
{
	switch(((task_t*)args)->type)
	{
	case FLAT_COMBINING:
		fc_lock(&fcLock, &job, args);
		break;
	case CC_SYNCH:
		cc_synch_lock(&ccSynch, &job, args);
		break;
	}
}

void lock_test()
{
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

	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION);
	printf("ACTUAL %llu\n", global_counter);
}