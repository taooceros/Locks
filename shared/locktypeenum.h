//
// Created by hongtao on 2/14/23.
//

#include "enum_to_string.h"

BEGIN_ENUM(LOCK_TYPE)
{
	DECL_ENUM_ELEMENT_STR(FLAT_COMBINING, "Flat Combining")
		DECL_ENUM_ELEMENT_STR(FLAT_COMBINING_FAIR, "Flat Combining (fair)")
		DECL_ENUM_ELEMENT_STR(FLAT_COMBINING_FAIR_PQ, "Flat Combining (fair, priority queue)")
		DECL_ENUM_ELEMENT_STR(CC_SYNCH, "CC Synch")
		DECL_ENUM_ELEMENT_STR(RCL, "Remote Core Locking")
} END_ENUM(LOCK_TYPE)
