#include <common.h>
#include <cpuid.h>
#include <pthread.h>

#include <ccsynch.h>
#include <flatcombining.h>
#include <flatcombiningfair.h>
#include <rcl.h>
#include <ticket.h>

#include <rdtsc.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "flatcombiningfairpq.h"
#include "locktypeenum.h"

#define GENERATE_ENUM_STRINGS
#include "locktypeenum.h"

#undef GENERATE_ENUM_STRINGS


// #define THREAD_COUNT 40
#ifdef DEBUG
#	define EXP_DURATION 2
#else
#	define EXP_DURATION 2 
#endif

typedef unsigned long long ull;

volatile ull global_counter = 0;

// define lock
fc_lock_t counter_lock_fc;
fcf_lock_t counter_lock_fcf;
fcfpq_lock_t counter_lock_fcfpq;
cc_synch_t counter_lock_cc;
rcl_lock_t coutner_lock_rcl;
rcl_server_t rcl_server;
// spinlock
pthread_spinlock_t counter_lock_spin;
// mutex
pthread_mutex_t counter_lock_mutex;
// ticket lock
ticket_lock_t counter_lock_ticket;

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
		case FLAT_COMBINING_FAIR_PQ:
			fcfpq_lock(&counter_lock_fcfpq, &job, &task);
			break;
		case CC_SYNCH:
			cc_synch_lock(&counter_lock_cc, &job, task);
			break;
		case RCL:
			rcl_lock(&coutner_lock_rcl, &job, task);
			break;
		case SPIN_LOCK:
			pthread_spin_lock(&counter_lock_spin);
			job(task);
			pthread_spin_unlock(&counter_lock_spin);
			sched_yield();
			break;
		case MUTEX:
			pthread_mutex_lock(&counter_lock_mutex);
			job(task);
			pthread_mutex_unlock(&counter_lock_mutex);
			break;
		case TICKET_LOCK:
			ticket_lock(&counter_lock_ticket);
			job(task);
			ticket_unlock(&counter_lock_ticket);
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
	fprintf(output, "lock type,id,cpuid,loop,lock_acquires,lock_hold(ms),duration\n");
	return output;
}

void init_lock(LOCK_TYPE lockType, int ncpus)
{
	static bool start_rcl_server = false;

	switch(lockType)
	{
	case FLAT_COMBINING:
		fc_init(&counter_lock_fc);
		break;
	case FLAT_COMBINING_FAIR:
		fcf_init(&counter_lock_fcf);
		break;
	case CC_SYNCH:
		cc_synch_init(&counter_lock_cc);
		break;
	case RCL:
		if(!start_rcl_server)
		{
			start_rcl_server = true;
			rcl_server_init(&rcl_server, ncpus - 1);
			rcl_lock_init(&coutner_lock_rcl, &rcl_server);
		}
		break;
	case SPIN_LOCK:
		pthread_spin_init(&counter_lock_spin, PTHREAD_PROCESS_PRIVATE);
		break;
	case MUTEX:
		pthread_mutex_init(&counter_lock_mutex, NULL);
		break;
	case TICKET_LOCK:
		ticket_init(&counter_lock_ticket);
		break;
	case FLAT_COMBINING_FAIR_PQ:
		fcfpq_init(&counter_lock_fcfpq);
		break;
	}
}

void inner_lock_test(LOCK_TYPE lockType, bool verbose, int ncpus, int nthreads)
{
	init_lock(lockType, ncpus);

	char* output_name = get_output_name(lockType, ncpus);

	FILE* output = setup_output(output_name);

	global_counter = 0;

	pthread_t threads[nthreads];
	task_t tasks[nthreads];
	int stop __attribute__((aligned(64))) = 0;

	for(int i = 0; i < nthreads; i++)
	{
		tasks[i].stop = &stop;
		tasks[i].cs = (i % 2 ? 3 : 1);

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
	for(int i = 0; i < nthreads; ++i)
	{
		CPU_ZERO(&cpu_set);

		int cpu_id = lockType == RCL ? (i % (ncpus - 1)) : i % ncpus;
		CPU_SET(cpu_id, &cpu_set);
		pthread_attr_setaffinity_np(&attr, sizeof(cpu_set_t), &cpu_set);

		tasks[i].id = i;
		tasks[i].cpu = cpu_id;

		pthread_create(&threads[i], &attr, &worker, &tasks[i]);
	}

	sleep(EXP_DURATION);
	stop = 1;

	for(int i = 0; i < nthreads; i++)
	{
		pthread_join(threads[i], NULL);
	}

	if(verbose)
	{
		for(int i = 0; i < nthreads; i++)
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

	for(int i = 0; i < nthreads; ++i)
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
		inner_lock_test(lockType, verbose, ncpu, ncpu);
	}
}

int main()
{
//	 lock_test(MUTEX, true);
	// lock_test(SPIN_LOCK, true);
	// lock_test(TICKET_LOCK, true);
//	 lock_test(FLAT_COMBINING, true);
	 lock_test(FLAT_COMBINING_FAIR, true);
//	lock_test(FLAT_COMBINING_FAIR_PQ, true);
	// lock_test(CC_SYNCH, true);
	// lock_test(RCL, true);
}
