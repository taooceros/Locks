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
	void* args;
	rcl_lock_t* lock;
};

struct rcl_thread_t
{
	rcl_server_t* server;
	int timestamp;
	int is_servicing;
};

typedef struct rcl_thread_node_t
{
	rcl_thread_t* thread;
	struct rcl_thread_node_t* next;
} rcl_thread_node_t;

struct rcl_server_t
{
	rcl_thread_node_t* head;
	lockfree_stack_t* prepared_threads;
	atomic_int_fast32_t num_free_threads;
	atomic_int_fast32_t num_serving_threads;
	int timestamp;
	bool is_alive;
	rcl_request_t* requests;
};

#endif //LOCKS_RCL_H
