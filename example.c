#define _GNU_SOURCE

#include <common.h>
#include <cpuid.h>
#include <pthread.h>

#include <ccsynch.h>
#include <flatcombining.h>
#include <flatcombiningfair.h>
#include <rcl.h>

#include <rdtsc.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "locktypeenum.h"

#define GENERATE_ENUM_STRINGS
#include "locktypeenum.h"

#undef GENERATE_ENUM_STRINGS

#define THREAD_COUNT 40

#define EXP_DURATION 2

typedef unsigned long long ull;

volatile ull global_counter = 0;

fc_lock_t counter_lock_fc;
fcf_lock_t counter_lock_fcf;
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
	int cpu;
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
		case FLAT_COMBINING_FAIR:
			fcf_lock(&counter_lock_fcf, &job, task);
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

char* get_output_name(LOCK_TYPE type, int ncpus)
{
	char* name = malloc(strlen(GetStringLOCK_TYPE(type)) + 16);
	strcpy(name, GetStringLOCK_TYPE(type));

	char ncpus_buf[16];

	sprintf(ncpus_buf, "_%d.csv", ncpus);
	strcat(name, ncpus_buf);
	return name;
}

FILE* setup_output(const char* name)
{
	FILE* output = fopen(name, "wb+");
	fprintf(output,
			"lock type,id,cpuid,loop,lock_acquires,lock_hold(ms),inner_test_lock duration\n");
	return output;
}

void inner_lock_test(LOCK_TYPE lockType, bool verbose, int ncpus)
{
	static bool start_rcl_server = false;

	char* output_name = get_output_name(lockType, ncpus);

	FILE* output = setup_output(output_name);

	if(lockType == RCL && !start_rcl_server)
	{
		start_rcl_server = true;
		rcl_server_init(&rcl_server, ncpus - 1);
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

		tasks[i].id = i;
		tasks[i].lock_type = lockType;
		tasks[i].loop_in_cs = 0;
		tasks[i].lock_acquires = 0;
		tasks[i].lock_hold = 0;
	}

	pthread_attr_t attr;
	cpu_set_t cpu_set;

	pthread_attr_init(&attr);

	// TODO: modify to accommodate NUMA
	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		CPU_ZERO(&cpu_set);

		int cpu_id = lockType == RCL ? (i % (ncpus - 1)) : i % ncpus;
		CPU_SET(cpu_id, &cpu_set);

		tasks[i].id = i;
		tasks[i].cpu = cpu_id;

		pthread_create(&threads[i], &attr, &worker, &tasks[i]);
	}

	sleep(EXP_DURATION);
	stop = 1;

	for(int i = 0; i < THREAD_COUNT; i++)
	{
		pthread_join(threads[i], NULL);
	}

	if(verbose)
	{
		for(int i = 0; i < THREAD_COUNT; i++)
		{
			fprintf(output,
					"%s,%d,%d,%llu,%llu,%.3f,%d\n",
					GetStringLOCK_TYPE(lockType),
					tasks[i].id,
					tasks[i].cpu,
					tasks[i].loop_in_cs,
					tasks[i].lock_acquires,
					tasks[i].lock_hold / (double)(CYCLE_PER_US * 1000),
					EXP_DURATION);
		}
	}

	ull loopResult = 0;

	for(int i = 0; i < THREAD_COUNT; ++i)
	{
		loopResult += tasks[i].loop_in_cs;
	}

	fclose(output);

	free(output_name);
}

void lock_test(LOCK_TYPE lockType, bool verbose)
{
	int ncpu = sysconf(_SC_NPROCESSORS_CONF) * 2;

	while((ncpu >>= 1) > 1)
	{
		printf("testing %s for ncpu %d\n", GetStringLOCK_TYPE(lockType), ncpu);
		inner_lock_test(lockType, verbose, ncpu);
	}
}

int main()
{
	fc_init(&counter_lock_fc);
	fcf_init(&counter_lock_fcf);
	cc_synch_init(&counter_lock_cc);

	// lock_test(FLAT_COMBINING, true);
	lock_test(FLAT_COMBINING_FAIR, true);
	// lock_test(CC_SYNCH, true);
	// lock_test(RCL, true);
}