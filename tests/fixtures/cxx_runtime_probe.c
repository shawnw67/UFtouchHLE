/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

extern int __cxa_guard_acquire(uint64_t *);
extern void __cxa_guard_abort(uint64_t *);
extern void __cxa_guard_release(uint64_t *);
extern int __cxa_atexit(void (*)(void *), void *, void *);
extern void *__dynamic_cast(const void *, const void *, const void *, int);

static void c_first(void) { puts("HOST_EXIT_FIRST"); }
static void c_last(void) { puts("HOST_EXIT_LAST"); }
static void cxx_middle(void *unused) { puts("HOST_EXIT_MIDDLE"); }

int main(void) {
  uint64_t guard = 0;
  if (!__cxa_guard_acquire(&guard))
    return 1;
  __cxa_guard_abort(&guard);
  if (!__cxa_guard_acquire(&guard))
    return 2;
  __cxa_guard_release(&guard);
  if (__cxa_guard_acquire(&guard))
    return 3;

  struct typeinfo {
    void *vtable;
    const char *name;
  } source = {NULL, "Base"}, target = {NULL, "Derived"};
  uintptr_t vtable[3] = {0, (uintptr_t)&target, 0};
  void *object_vtable = &vtable[2];
  void *object = &object_vtable;
  if (__dynamic_cast(object, &source, &target, 0) != object ||
      __dynamic_cast(object, &source, &target, -1) ||
      __dynamic_cast(NULL, &source, &target, 0))
    return 4;
  if (atexit(c_first) || __cxa_atexit(cxx_middle, NULL, NULL) || atexit(c_last))
    return 5;
  puts("HOST_CXX_PROBE_OK");
  exit(0);
}
