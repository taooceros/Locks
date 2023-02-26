//
// Created by hongtao on 2/14/23.
//

#include "enum_to_string.h"

BEGIN_ENUM(LOCK_TYPE){
	// spin lock
	DECL_ENUM_ELEMENT_STR(SPIN_LOCK, "Spin Lock")
	// mutex
	DECL_ENUM_ELEMENT_STR(MUTEX, "Mutex")
	// ticket lock
	DECL_ENUM_ELEMENT_STR(TICKET_LOCK, "Ticket Lock")
	// flat combining
	DECL_ENUM_ELEMENT_STR(FLAT_COMBINING, "Flat Combining")
	// flat combining fair (banning)
	DECL_ENUM_ELEMENT_STR(FLAT_COMBINING_FAIR, "Flat Combining (fair)")
	// flat combining fair (priority queue)
	DECL_ENUM_ELEMENT_STR(FLAT_COMBINING_FAIR_PQ, "Flat Combining (fair, priority queue)")
	// cc synch
	DECL_ENUM_ELEMENT_STR(CC_SYNCH, "CC Synch")
	// remote core locking
	DECL_ENUM_ELEMENT_STR(RCL, "Remote Core Locking")} END_ENUM(LOCK_TYPE)
