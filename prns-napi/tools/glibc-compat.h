#ifndef PRNS_NAPI_GLIBC_COMPAT_H
#define PRNS_NAPI_GLIBC_COMPAT_H

#include <features.h>

#ifdef __GLIBC_USE_C23_STRTOL
#undef __GLIBC_USE_C23_STRTOL
#define __GLIBC_USE_C23_STRTOL 0
#endif

#ifdef __GLIBC_USE_C2X_STRTOL
#undef __GLIBC_USE_C2X_STRTOL
#define __GLIBC_USE_C2X_STRTOL 0
#endif

#endif
