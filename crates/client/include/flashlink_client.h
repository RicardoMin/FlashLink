#ifndef FLASHLINK_CLIENT_H
#define FLASHLINK_CLIENT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

uint32_t fl_client_abi_version(void);
/* Positive attempt ID, -1 invalid address, -2 busy, -3 unavailable. */
int64_t fl_client_begin(const uint8_t *host, size_t length);
/* Copies UTF-8 without a trailing NUL; returns byte count or 0. */
int32_t fl_client_target(int64_t attempt, uint8_t *out, size_t capacity);
/* 0 idle, 1 launching, 2 native player UI foreground, -3 unavailable. */
int32_t fl_client_phase(void);
/* 1 opened, 2 returned, 3 failed; returns 1 when accepted, 0 otherwise. */
int32_t fl_client_event(int64_t attempt, int32_t event);

#ifdef __cplusplus
}
#endif
#endif
