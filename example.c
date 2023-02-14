#define _GNU_SOURCE

#include <common.h>
#include <cpuid.h>
#include <pthread.h>

#include <ccsynch.h>
#include <flatcombining.h>
#include <rcl.h>

#include <rdtsc.h>
#include <sched.h>
#include <stdio.h>
#include <unistd.h>

#include "example.h"

#define GENERATE_ENUM_STRINGS
#include "example.h"

#undef GENERATE_ENUM_STRINGS

#define THREAD_COUNT 128

#define DURATION_SCALE 10

typedef unsigned long long ull;

volatile ull global_counter = 0;

fc_lock_t counter_lock_fc;
cc_synch_t counter_lock_cc;
rcl_lock_t coutner_lock_rcl;
rcl_server_t rcl_server;

typedef struct
{
	volatile int* stop;
	pthread_t thread;
	int priority;
	int lock_type;
#ifdef FAIRLOCK
//	int weight;
#endif
	int id;
	double cs;
	int ncpu;
	// outputs
	ull loop_in_cs;
	ull lock_acquires;
	ull lock_hold;
} task_t;

void* job(void* arg)
{
	task_t* task = arg;

	task->lock_acquires++;

	const ull delta = CYCLE_PER_US * task->cs;
	ull initial = rdtscp();
	ull now;
	ull then = initial + delta;

	do
	{
		task->loop_in_cs++;
		global_counter++;
	} while((now = rdtscp()) < then);

	task->lock_hold += now - initial;

	return 0;
}

void* worker(void* arg)
{
	task_t* task = arg;

	LOCK_TYPE lockType = task->lock_type;

	if(lockType == RCL)
	{
		rcl_register_client(&rcl_server);
	}

	do
	{
		switch(lockType)
		{
		case FLAT_COMBINING:
			fc_lock(&counter_lock_fc, &job, task);
			break;
		case CC_SYNCH:
			cc_synch_lock(&counter_lock_cc, &job, task);
			break;
		case RCL:
			rcl_lock(&coutner_lock_rcl, &job, task);
			break;
		}
	} while(!*task->stop);

	return NULL;
}

void test_lock(LOCK_TYPE lockType, bool verbose, int duration)
{
	int num_cpus = sysconf(_SC_NPROCESSORS_ONLN);

	static bool start_rcl_server = false;

	if(lockType == RCL && !start_rcl_server)
	{
		start_rcl_server = true;
		rcl_server_init(&rcl_server, num_cpus - 1);
		rcl_lock_init(&coutner_lock_rcl, &rcl_server);
	}

	global_counter = 0;

	pthread_t threads[THREAD_COUNT];
	task_t tasks[THREAD_COUNT];
	int stop __attribute__((aligned(64))) = 0;

	for(int i = 0; i < THREAD_COUNT; i++)
	{
		tasks[i].stop = &stop;
		tasks[i].cs = (i % 2 ? 300 : 100);

		int priority = 1;
		tasks[i].priority = priority;
#ifdef FAIRLOCK
//		int weight = prio_to_weight[priority + 20];
//		tasks[i].weight = weight;
//		tot_weight += weight;
#endif

		tasks[i].ncpu = num_cpus;
		tasks[i].id = i;
		tasks[i].lock_type = lockType;
		tasks[i].loop_in_cs = 0;
		tasks[i].lock_acquires = 0;
		tasks[i].lock_hold = 0;
	}

	pthread_attr_t attr;
	cpu_set_t cpu_set;

	pthread_attr_init(&attr);

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		CPU_ZERO(&cpu_set);

		if(lockType == RCL)
			CPU_SET(i % (tasks[i].ncpu - 1), &cpu_set);
		else
			CPU_SET(i % tasks[i].ncpu, &cpu_set);

		tasks[i].id = i;
		pthread_create(&threads[i], &attr, &worker, &tasks[i]);
	}

	sleep(duration);
	stop = 1;

	for(int i = 0; i < THREAD_COUNT; i++)
	{
		pthread_join(threads[i], NULL);
	}

	if(verbose)
	{
		for(int i = 0; i < THREAD_COUNT; i++)
		{
			printf("%s,%d,%llu,%llu,%.3f,%d\n",
				   GetStringLOCK_TYPE(lockType),
				   tasks[i].id,
				   tasks[i].loop_in_cs,
				   tasks[i].lock_acquires,
				   tasks[i].lock_hold / (float)(CYCLE_PER_US * 1000),
				   duration);
		}
	}

	ull loopResult = 0;

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		loopResult += tasks[i].loop_in_cs;
	}
}

int main()
{
	FILE* fp = fopen("../output/result.txt", "wb");
	dup2(fileno(fp), STDOUT_FILENO);

	printf("lock type,id,loop,lock_acquires,lock_hold(ms),test duration\n");

	fc_init(&counter_lock_fc);
	cc_synch_init(&counter_lock_cc);

	int counter = 0;
	while(counter++ < 10)
		test_lock(FLAT_COMBINING, true, counter * DURATION_SCALE);

	counter = 0;
	while(counter++ < 10)
		test_lock(CC_SYNCH, true, counter * DURATION_SCALE);

	// rcl need to be tested at the end because it occupies a core as server
	counter = 0;
	while(counter++ < 10)
		test_lock(RCL, true, counter * DURATION_SCALE);

	fclose(fp);
}