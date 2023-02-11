//
// Created by 1 on 2/6/2023.
//

#ifndef LOCKS_RCL_H
#define LOCKS_RCL_H

#include <shared.h>
#include <stdatomic.h>
struct rcl_lock_t;
struct rcl_request_t;
struct rcl_thread_t;
struct rcl_server_t;

typedef struct rcl_lock_t rcl_lock_t;
typedef struct rcl_request_t rcl_request_t;
typedef struct rcl_thread_t rcl_thread_t;
typedef struct rcl_server_t rcl_server_t;

struct rcl_lock_t
{
	rcl_server_t* server;
	int holder;
};

struct rcl_request_t
{
	func_ptr_t delegate;
	void* context;
	rcl_lock_t* lock;
	int real_me;
};

struct rcl_thread_t
{
	rcl_server_t* server;
	int timestamp;
	int is_servicing;
	pthread_t pthread;
};

typedef struct rcl_thread_node_t
{
	rcl_thread_t* thread;
	struct rcl_thread_node_t* next;
} rcl_thread_node_t;

struct rcl_server_t
{
	rcl_thread_node_t* threads;
	lockfree_stack_t* prepared_threads;
	atomic_int_fast32_t num_free_threads;
	atomic_int_fast32_t num_serving_threads;
	int timestamp;
	bool is_alive;
	int cpu;
	pthread_t management_thread;
	int management_alive;
	rcl_request_t requests[128];
};
void rcl_lock_init(rcl_lock_t* l, rcl_server_t* s);

void rcl_server_init(rcl_server_t* s, int cpu);

void rcl_register_client(rcl_server_t* s);

void* rcl_lock(rcl_lock_t* l, func_ptr_t delegate, void* context);

#endif //LOCKS_RCL_H
