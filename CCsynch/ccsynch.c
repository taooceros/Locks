#include <stdbool.h>
#include <stddef.h>

typedef struct cc_thread_node
{
	void* req;
	void* ret;
	bool wait;
	bool completed;
	struct cc_thread_node* next;
} node;

typedef struct cc_synch
{
	node* Tail;
} cc_synch_t;

void cc_synch_init(cc_synch_t* cc)
{
	cc->Tail->req = NULL;
	cc->Tail->ret = NULL;
	cc->Tail->wait = false;
	cc->Tail->completed = false;
	cc->Tail->next = NULL;
}

void* cc_synch(void* req)
{
	node* nextNode;
	node* currentNode;
	node* tmpNode;
	node* tmpNodeNext;

    int counter = 0;

    
}