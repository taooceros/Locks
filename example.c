#define _GNU_SOURCE

#include <ccsynch.h>
#include <common.h>
#include <cpuid.h>
#include <flatcombining.h>
#include <pthread.h>
#include <rdtsc.h>
#include <sched.h>
#include <stdio.h>
#include <unistd.h>

volatile int global_counter = 0;

fc_lock_t counter_lock_fc;
cc_synch_t counter_lock_cc;

typedef unsigned long long ull;
typedef struct
{
	volatile int* stop;
	pthread_t thread;
	int priority;
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
} task_t __attribute__((aligned(64)));

void* job(void* arg)
{
	task_t* task = arg;

	task->lock_acquires++;

	const ull delta = CYCLE_PER_US * task->cs;
	ull initial = rdtscp();
	ull now;
	ull then = initial + delta;

	int counter = 0;
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
	int counter = 0;
	task_t* task = arg;

	if(task->ncpu != 0)
	{
		cpu_set_t cpuset;
		CPU_ZERO(&cpuset);
		int ret;
		for(int i = 0; i < task->ncpu; i++)
		{
			if(i < 8 || i >= 24)
				CPU_SET(i, &cpuset);
			else if(i < 16)
				CPU_SET(i + 8, &cpuset);
			else
				CPU_SET(i - 8, &cpuset);
		}
		ret = pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
		if(ret != 0)
		{
			perror("pthread_set_affinity_np");
			exit(-1);
		}
	}

	ull now;
	do
	{
		fc_lock(&counter_lock_fc, &job, task);
	} while(!*task->stop);

	return NULL;
}

int main()
{
	fc_init(&counter_lock_fc);
	cc_synch_init(&counter_lock_cc);

	const int thread_count = 12;

	pthread_t threads[thread_count];
	task_t tasks[thread_count];
	int stop __attribute__((aligned(64))) = 0;

	for(int i = 0; i < thread_count; i++)
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

		tasks[i].ncpu = 12;
		tasks[i].id = i;

		tasks[i].loop_in_cs = 0;
		tasks[i].lock_acquires = 0;
		tasks[i].lock_hold = 0;
		pthread_create(&threads[i], NULL, &worker, &tasks[i]);
	}

	sleep(5);
	stop = 1;

	for(int i = 0; i < thread_count; i++)
	{
		pthread_join(threads[i], NULL);
	}

	for(int i = 0; i < thread_count; i++)
	{
		printf("id %02d "
			   "loop %10llu "
			   "lock_acquires %8llu "
			   "lock_hold(ms) %10.3f \n",
			   tasks[i].id,
			   tasks[i].loop_in_cs,
			   tasks[i].lock_acquires,
			   tasks[i].lock_hold / (float)(CYCLE_PER_US * 1000));
#if defined(FAIRLOCK) && defined(DEBUG)
		flthread_info_t* info = pthread_getspecific(lock.flthread_info_key);
		printf("  slice %llu\n"
			   "  own_slice_wait %llu\n"
			   "  prev_slice_wait %llu\n"
			   "  runnable_wait %llu\n"
			   "  next_runnable_wait %llu\n"
			   "  succ_wait %llu\n"
			   "  reenter %llu\n"
			   "  banned(actual) %llu\n"
			   "  banned %llu\n"
			   "  elapse %llu\n",
			   task->lock_acquires - info->stat.reenter,
			   info->stat.own_slice_wait,
			   info->stat.prev_slice_wait,
			   info->stat.runnable_wait,
			   info->stat.next_runnable_wait,
			   info->stat.succ_wait,
			   info->stat.reenter,
			   info->stat.banned_time,
			   info->banned_until - info->stat.start,
			   info->start_ticks - info->stat.start);
#endif
	}

	printf("Global Result %d\n", global_counter);
}