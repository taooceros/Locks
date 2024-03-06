#include "shared.h"
#include "ccsynch.h"
#include <stdatomic.h>

static void free_key(void* key)
{
	free(key);
}

static inline void node_init(node_t* node)
{
	node->result = NULL;
	node->wait = false;
	node->completed = false;
	node->next = NULL;
}

void cc_synch_init(cc_synch_t* cc)
{
	cc->Tail = malloc(sizeof(node_t));
	node_init(cc->Tail);
	pthread_key_create(&cc->ccthread_info_key, &free_key);
}

static inline node_t* retrieveNode(cc_synch_t* lock)
{
	node_t* node = pthread_getspecific(lock->ccthread_info_key);

	if(node == NULL)
	{
		node = malloc(sizeof(node_t));
		node_init(node);
		pthread_setspecific(lock->ccthread_info_key, node);
	}

	return node;
}

void* cc_synch_lock(cc_synch_t* lock, func_ptr_t delegate, void* args)
{
	node_t* nextNode;
	node_t* currentNode;
	node_t* tmpNode;
	node_t* tmpNodeNext;

	int counter = 0;

	node_t* threadNode = retrieveNode(lock);

	nextNode = threadNode;

	nextNode->next = NULL;
	nextNode->wait = true;
	nextNode->completed = false;

	currentNode = atomic_exchange(&lock->Tail, nextNode);

	currentNode->request.delegate = delegate;
	currentNode->request.args = args;
	currentNode->next = nextNode;

	pthread_setspecific(lock->ccthread_info_key, currentNode);

	while(currentNode->wait)
		_mm_pause();

	if(currentNode->completed)
		return currentNode->result;

	// become the combiner

	tmpNode = currentNode;

	const int h = 64;

	while(tmpNode->next != NULL && counter < h)
	{
		counter++;
		tmpNodeNext = tmpNode->next;

		tmpNode->result = tmpNode->request.delegate(tmpNode->request.args);
		tmpNode->completed = true;
		tmpNode->wait = false;
		tmpNode = tmpNodeNext;
	}

	tmpNode->wait = false;
	return currentNode->result;
}