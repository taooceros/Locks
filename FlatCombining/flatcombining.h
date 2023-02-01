#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

typedef struct thread_node
{
	int age;
	bool active;
	void* (*delegate)(void*);
	void* args;
	void* response;
	struct thread_node* next;
} thread_node_t;

typedef struct
{
	int pass;
	int flag;
	thread_node_t* head;
	pthread_key_t fcthread_info_key;
} fc_lock_t;

void fc_init(fc_lock_t* lock);

void* fc_lock(fc_lock_t* lock, void* (*func_ptr)(void*), void* arg);