#include <assert.h>
#include <ccsynch.h>
#include <common.h>
#include <flatcombining.h>

#define ITERATION 50000
#define THREAD_COUNT 32

typedef enum
{
	FLAT_COMBINING,
	CC_SYNCH
} LOCK_TYPE;

typedef struct
{
	LOCK_TYPE type;
} task_t;

u_int64_t volatile global_counter = 0;

fc_lock_t fcLock;
cc_synch_t ccSynch;

void* job(void* arg)
{
	u_int32_t counter = 0;
	while(counter++ < ITERATION)
		global_counter++;
}

void* worker(void* args)
{
	switch(((task_t*)args)->type)
	{
	case FLAT_COMBINING:
		fc_lock(&fcLock, &job, NULL);
	case CC_SYNCH:
		cc_synch_lock(&ccSynch, &job, NULL);
	}
}

int main()
{
	fc_init(&fcLock);
	cc_synch_init(&ccSynch);

	task_t task1;
	task1.type = FLAT_COMBINING;

	pthread_t pthreads[THREAD_COUNT];

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		pthread_create(&pthreads[i], NULL, &worker, &task1);
	}

	for(int i = 0; i < sizeof(pthreads) / sizeof(pthreads[0]); ++i)
	{
		pthread_join(pthreads[i], NULL);
	}

	printf("EXPECTED %d\n", THREAD_COUNT * ITERATION);
	printf("ACTUAL %ld\n", global_counter);
}