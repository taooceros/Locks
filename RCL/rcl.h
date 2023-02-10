//
// Created by 1 on 2/6/2023.
//

#ifndef LOCKS_RCL_H
#define LOCKS_RCL_H

#include "shared.h"
#include <stdbool.h>
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
	bool is_locked;
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
	bool is_servicing;
};

typedef struct rcl_thread_node_t
{
	rcl_thread_t* thread;
	struct rcl_thread_node_t* next;
} rcl_thread_node_t;

struct rcl_server_t
{
	rcl_thread_node_t *head;

};

#endif //LOCKS_RCL_H
