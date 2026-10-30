/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

// This file contains the command-line automated tests. tests/integration.rs
// runs these automatically.

#include <CoreFoundation/CFBase.h>
#include <CoreFoundation/CFBundle.h>
#include <CoreFoundation/CFDictionary.h>
#include <CoreFoundation/CFLocale.h>
#include <CoreFoundation/CFNumber.h>
#include <CoreFoundation/CFString.h>
#include <CoreFoundation/CFURL.h>
#include <CoreFoundation/CFUUID.h>
#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fenv.h>
#include <fnmatch.h>
#include <glob.h>
#include <locale.h>
#include <mach/kern_return.h>
#include <mach/thread_info.h>
#include <malloc/malloc.h>
#include <math.h>
#include <pthread.h>
#include <semaphore.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <uuid/uuid.h>
#include <wchar.h>

#import "SyncTester.h"

// TODO: include from <mach/thread_act.h> once available in the common-sdk
extern kern_return_t thread_suspend(mach_port_t target_act);
extern kern_return_t thread_resume(mach_port_t target_act);
extern kern_return_t thread_info(mach_port_t target_act, natural_t flavor,
                                 integer_t *thread_info_out,
                                 natural_t *thread_info_out_cnt);

// Declare test functions from other files.

int test_AutoreleasePool(void);    // AutoReleasePoolTest.m
int test_CGAffineTransform(void);  // CGAffineTransform.c
int test_RespondsToSelector(void); // RespondsToSelector.m
int test_Initialize(void);         // Initialize.m

#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
int test_cpp_virtual_inheritance(void); // CppVirtualInheritance.cpp
#endif

// === Main code ===

int test_CGGeometry() {
  CGRect testRect;
  testRect.origin.x = 2.0;
  testRect.origin.y = 3.0;
  testRect.size.width = 100.0;
  testRect.size.height = 200.0;

  if (!(CGRectGetMinX(testRect) == testRect.origin.x &&
        CGRectGetMinX(testRect) == 2.0))
    return -1;
  if (!(CGRectGetMaxX(testRect) == testRect.origin.x + testRect.size.width &&
        CGRectGetMaxX(testRect) == 102.0))
    return -2;

  if (!(CGRectGetMinY(testRect) == testRect.origin.y &&
        CGRectGetMinY(testRect) == 3.0))
    return -3;

  if (!(CGRectGetMaxY(testRect) == testRect.origin.y + testRect.size.height &&
        CGRectGetMaxY(testRect) == 203.0))
    return -4;

  if (!(CGRectGetHeight(testRect) == testRect.size.height))
    return -5;

  if (!(CGRectGetWidth(testRect) == testRect.size.width))
    return -6;

  return 0;
}

int int_compar(const void *a, const void *b) { return *(int *)a - *(int *)b; }

int sort_and_check(int nel, int *arr, int *expected_arr) {
  qsort(arr, nel, sizeof(int), &int_compar);
  return memcmp(arr, expected_arr, nel * sizeof(int));
}

int test_qsort() {
  // empty
  int res = sort_and_check(0, (int[]){}, (int[]){});
  if (res != 0)
    return -1;
  // one element
  res = sort_and_check(1, (int[]){42}, (int[]){42});
  if (res != 0)
    return -1;
  // even size
  res = sort_and_check(4, (int[]){4, 3, 2, 1}, (int[]){1, 2, 3, 4});
  if (res != 0)
    return -1;
  // odd size
  res =
      sort_and_check(5, (int[]){1, -1, 2, 1024, 4}, (int[]){-1, 1, 2, 4, 1024});
  if (res != 0)
    return -1;
  return 0;
}

char *str_format(const char *format, ...) {
  char *str = malloc(256);
  if (str == NULL) {
    exit(EXIT_FAILURE);
  }
  va_list args;
  va_start(args, format);
  vsnprintf(str, 256, format, args);
  va_end(args);
  return str;
}

// A description callback overwrites live argument registers during %@.
int test_uuid_c_values() {
  uuid_t value, copy;
  uuid_string_t string;
  const char *canonical = "550E8400-E29B-41D4-A716-446655440000";
  if (uuid_parse(canonical, value))
    return 1;
  uuid_copy(copy, value);
  if (uuid_compare(copy, value) || uuid_is_null(value))
    return 2;
  uuid_unparse(value, string);
  if (strcmp(string, canonical))
    return 3;
  uuid_unparse_lower(value, string);
  if (strcmp(string, "550e8400-e29b-41d4-a716-446655440000"))
    return 4;
  if (uuid_parse("550E8400E29B41D4A716446655440000", copy) != -1 ||
      uuid_parse("zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz", copy) != -1 ||
      uuid_compare(copy, value))
    return 5;
  uuid_generate_random(copy);
  if ((copy[6] >> 4) != 4 || (copy[8] & 0xc0) != 0x80)
    return 6;
  uuid_clear(copy);
  return uuid_is_null(copy) ? 0 : 7;
}

int test_named_semaphore_reopen() {
  const char *name = "hyper_reopen";
  sem_t *a = sem_open(name, O_CREAT | O_EXCL, 0600, 1);
  if (a == SEM_FAILED)
    return 1;
  sem_t *b = sem_open(name, 0);
  if (a != b)
    return 2;
  errno = 0;
  if (sem_open(name, O_CREAT | O_EXCL, 0600, 1) != SEM_FAILED ||
      errno != EEXIST)
    return 3;
  if (sem_wait(b) || sem_post(a))
    return 4;
  if (sem_close(a) || sem_unlink(name))
    return 5;
  errno = 0;
  return sem_open(name, 0) == SEM_FAILED && errno == ENOENT ? 0 : 6;
}

int test_variadic_callback_snapshot() {
  NSString *format = [NSString stringWithUTF8String:"%@ %d %d"];
  NSString *object = [NSString stringWithUTF8String:"object"];
  NSString *value = [NSString stringWithFormat:format, object, 17, 29];
  return [value isEqualToString:[NSString stringWithUTF8String:"object 17 29"]]
             ? 0
             : 1;
}

int test_semaphore_lifecycle_and_unlink() {
  const char *name = "uf_lifetime";
  sem_t *old = sem_open(name, O_CREAT | O_EXCL, 0600, 2);
  if (old == SEM_FAILED || sem_wait(old))
    return 1;
  errno = EDOM;
  if (sem_close(old) || errno != EDOM)
    return 2;
  old = sem_open(name, 0);
  int value = -99;
  if (old == SEM_FAILED || sem_getvalue(old, &value) || value != 1)
    return 3;
  if (sem_destroy(old) != -1 || errno != EINVAL)
    return 4;
  if (sem_unlink(name))
    return 5;
  if (sem_unlink(name) != -1 || errno != ENOENT)
    return 6;
  if (sem_open(name, 0) != SEM_FAILED || errno != ENOENT)
    return 7;
  sem_t *fresh = sem_open(name, O_CREAT | O_EXCL, 0600, 7);
  if (fresh == SEM_FAILED || fresh == old || sem_post(old))
    return 8;
  if (sem_getvalue(old, &value) || value != 2 || sem_getvalue(fresh, &value) ||
      value != 7)
    return 9;
  if (sem_close(old) || sem_close(fresh) || sem_unlink(name))
    return 10;
  return 0;
}

int test_semaphore_error_boundaries() {
  sem_t *bad[] = {NULL, SEM_FAILED};
  int value = 123;
  for (unsigned i = 0; i < 2; ++i) {
    if (sem_post(bad[i]) != -1 || errno != EINVAL || sem_wait(bad[i]) != -1 ||
        errno != EINVAL || sem_trywait(bad[i]) != -1 || errno != EINVAL ||
        sem_close(bad[i]) != -1 || errno != EINVAL ||
        sem_destroy(bad[i]) != -1 || errno != EINVAL ||
        sem_getvalue(bad[i], &value) != -1 || errno != EINVAL || value != 123)
      return 1;
  }
  sem_t local;
  if (sem_init((sem_t *)1, 0, 0) != -1 || errno != EINVAL)
    return 12;
  if (sem_init(&local, 1, 0) != -1 || errno != ENOTSUP)
    return 2;
  if (sem_init(&local, 0, 0))
    return 3;
  if (sem_init(&local, 0, 1) != -1 || errno != EBUSY)
    return 4;
  if (sem_close(&local) != -1 || errno != EINVAL)
    return 5;
  if (sem_getvalue(&local, NULL) != -1 || errno != EFAULT)
    return 6;
  if (sem_trywait(&local) != -1 || errno != EAGAIN)
    return 7;
  errno = EDOM;
  if (sem_post(&local) || errno != EDOM || sem_wait(&local) || errno != EDOM ||
      sem_destroy(&local) || errno != EDOM)
    return 8;
  if (sem_post(&local) != -1 || errno != EINVAL)
    return 9;
  if (sem_open("", O_CREAT, 0600, 0) != SEM_FAILED || errno != EINVAL)
    return 10;
  if (sem_unlink(NULL) != -1 || errno != EFAULT)
    return 11;
  return 0;
}

int test_semaphore_counter_limits() {
  const char *name = "uf_overflow";
  if (sem_open(name, O_CREAT, 0600, SEM_VALUE_MAX + 1u) != SEM_FAILED ||
      errno != EINVAL)
    return 1;
  sem_t *sem = sem_open(name, O_CREAT | O_EXCL, 0600, SEM_VALUE_MAX);
  if (sem == SEM_FAILED)
    return 2;
  int value;
  if (sem_post(sem) != -1 || errno != EOVERFLOW || sem_getvalue(sem, &value) ||
      value != SEM_VALUE_MAX)
    return 3;
  if (sem_trywait(sem) || sem_post(sem))
    return 4;
  if (sem_close(sem) || sem_unlink(name))
    return 5;
  return 0;
}

// Keep this fixture buildable with the pre-GCD common SDK as well.
extern void *dispatch_semaphore_create(long value);
extern long dispatch_semaphore_wait(void *semaphore,
                                    unsigned long long timeout);
extern long dispatch_semaphore_signal(void *semaphore);
extern void dispatch_release(void *object);

int test_dispatch_semaphore_independent_limit() {
  void *semaphore = dispatch_semaphore_create(40000);
  if (!semaphore)
    return 1;
  int failed = dispatch_semaphore_wait(semaphore, 0) != 0 ||
               dispatch_semaphore_signal(semaphore) != 0;
  dispatch_release(semaphore);
  return failed;
}

static sem_t *uf_wait_target;
static sem_t *uf_wait_ready;
static void *uf_wait_worker(void *unused) {
  sem_post(uf_wait_ready);
  sem_wait(uf_wait_target);
  return NULL;
}

int test_semaphore_waiter_cleanup() {
  for (int named = 0; named < 2; ++named) {
    sem_t local;
    uf_wait_target =
        named ? sem_open("uf_wait", O_CREAT | O_EXCL, 0600, 0) : &local;
    if (uf_wait_target == SEM_FAILED || (!named && sem_init(&local, 0, 0)))
      return 1;
    uf_wait_ready = sem_open("uf_ready", O_CREAT | O_EXCL, 0600, 0);
    if (uf_wait_ready == SEM_FAILED)
      return 2;
    pthread_t worker;
    if (pthread_create(&worker, NULL, uf_wait_worker, NULL))
      return 3;
    if (sem_wait(uf_wait_ready))
      return 4;
    usleep(20000); // Let the worker enter its blocking wait.
    int result =
        named ? sem_close(uf_wait_target) : sem_destroy(uf_wait_target);
    if (result != -1 || errno != EBUSY)
      return 5;
    if (named && sem_unlink("uf_wait"))
      return 6;
    if (sem_post(uf_wait_target) || pthread_join(worker, NULL))
      return 7;
    int value = -99;
    if (sem_getvalue(uf_wait_target, &value) || value != 0)
      return 8;
    if (named ? sem_close(uf_wait_target) : sem_destroy(uf_wait_target))
      return 9;
    if (sem_close(uf_wait_ready) || sem_unlink("uf_ready"))
      return 10;
  }
  return 0;
}

// Runtime lookup keeps these tests buildable with the legacy common SDK.
int test_nsuuid_values() {
  id cls = NSClassFromString([NSString stringWithUTF8String:"NSUUID"]);
  if (cls == nil)
    return 1;
  unsigned char bytes[16] = {0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4,
                             0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00};
  NSString *canonical =
      [NSString stringWithUTF8String:"550E8400-E29B-41D4-A716-446655440000"];
  id a = [[cls alloc] initWithUUIDString:canonical];
  id b = [[cls alloc] initWithUUIDBytes:bytes];
  unsigned char result[16];
  [a getUUIDBytes:result];
  int failed = !a || !b || ![a isEqual:b] || [a hash] != [b hash] ||
               memcmp(bytes, result, 16) ||
               ![[a UUIDString] isEqualToString:canonical] ||
               [a isEqual:[NSString stringWithUTF8String:"not a UUID"]];
  id copy = [a copy];
  failed |= copy != a;
  [a release];
  failed |= ![copy isEqual:b];
  [copy release];
  [b release];
  id bad = [[cls alloc]
      initWithUUIDString:
          [NSString stringWithUTF8String:"💥0000000000000000000000000000"]];
  failed |= bad != nil;
  [bad release];
  return failed;
}

static CFUUIDRef uf_uuid_from_bytes(const unsigned char *b, int constant) {
  if (constant)
    return CFUUIDGetConstantUUIDWithBytes(NULL, b[0], b[1], b[2], b[3], b[4],
                                          b[5], b[6], b[7], b[8], b[9], b[10],
                                          b[11], b[12], b[13], b[14], b[15]);
  return CFUUIDCreateWithBytes(NULL, b[0], b[1], b[2], b[3], b[4], b[5], b[6],
                               b[7], b[8], b[9], b[10], b[11], b[12], b[13],
                               b[14], b[15]);
}

int test_cfuuid_byte_abi() {
  unsigned char bytes[16] = {1, 0, 0x80, 0x7f, 0x34, 0x12, 0xc0, 0xff,
                             0, 0, 0,    0x80, 0xff, 0xfe, 0xfd, 0xfc};
  CFUUIDBytes value;
  memcpy(&value, bytes, 16);
  CFUUIDRef a = uf_uuid_from_bytes(bytes, 0);
  CFUUIDRef (*volatile from_aggregate)(CFAllocatorRef, CFUUIDBytes) =
      CFUUIDCreateFromUUIDBytes;
  CFUUIDBytes (*volatile get_aggregate)(CFUUIDRef) = CFUUIDGetUUIDBytes;
  CFUUIDRef b = from_aggregate(kCFAllocatorSystemDefault, value);
  if (!a || !b || a != b || !CFEqual(a, b) || CFHash(a) != CFHash(b))
    return 1;
  struct {
    unsigned char before[4];
    CFUUIDBytes value;
    unsigned char after[4];
  } guarded;
  memset(&guarded, 0xa5, sizeof(guarded));
  guarded.value = get_aggregate(a);
  if (memcmp(&guarded.value, bytes, 16) ||
      memcmp(guarded.before, "\xa5\xa5\xa5\xa5", 4) ||
      memcmp(guarded.after, "\xa5\xa5\xa5\xa5", 4))
    return 2;
  CFRelease(a);
  guarded.value = get_aggregate(b);
  if (memcmp(&guarded.value, bytes, 16))
    return 3;
  CFRelease(b);
  return 0;
}

int test_cf_copy_description_ownership() {
  UInt8 bytes[16] = {0};
  CFUUIDRef uuid = uf_uuid_from_bytes(bytes, 0);
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CFStringRef description = CFCopyDescription(uuid);
  [pool drain];
  CFRelease(uuid);
  char output[64];
  int ok = description &&
           CFStringGetCString(description, output, sizeof(output),
                              kCFStringEncodingUTF8) &&
           !strcmp(output, "00000000-0000-0000-0000-000000000000");
  if (description)
    CFRelease(description);
  return ok ? 0 : 1;
}

int test_cfuuid_string_and_reference_lifetimes() {
  unsigned char bytes[16] = {0x55, 0x0e, 0x84, 0,    0xe2, 0x9b, 0x41, 0xd4,
                             0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0,    0};
  CFUUIDRef uuid = uf_uuid_from_bytes(bytes, 0);
  id pool = [[NSAutoreleasePool alloc] init];
  CFStringRef text = CFUUIDCreateString(NULL, uuid);
  [pool release];
  CFRelease(uuid);
  char output[64];
  if (!text ||
      !CFStringGetCString(text, output, sizeof(output),
                          kCFStringEncodingUTF8) ||
      strcmp(output, "550E8400-E29B-41D4-A716-446655440000"))
    return 1;
  CFRelease(text);
  for (int i = 0; i < 128; i++) {
    uuid = uf_uuid_from_bytes(bytes, 0);
    if (!uuid || CFGetRetainCount(uuid) != 1)
      return 2;
    CFUUIDBytes copy = CFUUIDGetUUIDBytes(uuid);
    if (memcmp(&copy, bytes, 16))
      return 3;
    CFRelease(uuid);
  }
  uuid = CFUUIDCreate(NULL);
  CFUUIDBytes random = CFUUIDGetUUIDBytes(uuid);
  const unsigned char *raw = (const unsigned char *)&random;
  if ((raw[6] & 0xf0) != 0x40 || (raw[8] & 0xc0) != 0x80)
    return 4;
  CFRelease(uuid);
  return 0;
}

int test_cfuuid_constants() {
  unsigned char bytes[16] = {0x01, 0xa2, 0x73, 0xb4, 0xc5, 0xd6, 0x47, 0xe8,
                             0xa9, 0xfa, 0x0b, 0x1c, 0x2d, 0x3e, 0x4f, 0xc0};
  CFUUIDRef root = uf_uuid_from_bytes(bytes, 1);
  CFIndex original_count = CFGetRetainCount(root);
  for (int i = 0; i < 128; i++)
    if (uf_uuid_from_bytes(bytes, 1) != root)
      return 1;
  if (CFGetRetainCount(root) != original_count)
    return 2;
  CFUUIDRef owned = uf_uuid_from_bytes(bytes, 0);
  if (owned != root)
    return 3;
  CFRelease(owned);
  CFUUIDBytes value = CFUUIDGetUUIDBytes(root);
  if (memcmp(&value, bytes, 16) || CFGetRetainCount(root) != original_count)
    return 4;
  owned = (CFUUIDRef)CFRetain(root);
  CFRelease(owned);
  bytes[15] = 0xc1;
  owned = uf_uuid_from_bytes(bytes, 0);
  CFUUIDRef promoted = uf_uuid_from_bytes(bytes, 1);
  if (owned != promoted)
    return 5;
  CFRelease(owned);
  value = CFUUIDGetUUIDBytes(promoted);
  if (memcmp(&value, bytes, 16) || uf_uuid_from_bytes(bytes, 1) != promoted)
    return 6;
  // Get results are borrowed constants; do not CFRelease them directly.
  return 0;
}

int test_cfuuid_historical_parser() {
  const char *canonical = "550E8400-E29B-41D4-A716-446655440000";
  CFStringRef text =
      CFStringCreateWithCString(NULL, canonical, kCFStringEncodingUTF8);
  CFUUIDRef a = CFUUIDCreateFromString(NULL, text);
  CFRelease(text);
  text = CFStringCreateWithCString(
      NULL, "{550e8400-e29b-41d4-a716-446655440000}", kCFStringEncodingUTF8);
  CFUUIDRef b = CFUUIDCreateFromString(NULL, text);
  CFRelease(text);
  if (!a || a != b)
    return 1;
  CFRelease(a);
  CFRelease(b);
  text = CFStringCreateWithCString(NULL, "", kCFStringEncodingUTF8);
  if (CFUUIDCreateFromString(NULL, text) || CFUUIDCreateFromString(NULL, NULL))
    return 2;
  CFRelease(text);
  UniChar units[37];
  units[0] = 0xd800; // A raw unpaired surrogate must not cause UTF-8 slicing.
  for (int i = 0; i < 36; i++)
    units[i + 1] = (unsigned char)canonical[i];
  text = CFStringCreateWithCharacters(NULL, units, 37);
  a = CFUUIDCreateFromString(NULL, text);
  CFRelease(text);
  CFStringRef roundtrip = CFUUIDCreateString(NULL, a);
  char output[64];
  if (!CFStringGetCString(roundtrip, output, sizeof(output),
                          kCFStringEncodingUTF8) ||
      strcmp(output, canonical))
    return 3;
  CFRelease(roundtrip);
  CFRelease(a);
  text = CFStringCreateWithCString(NULL, "55", kCFStringEncodingUTF8);
  a = CFUUIDCreateFromString(NULL, text);
  CFRelease(text);
  CFUUIDBytes partial = CFUUIDGetUUIDBytes(a);
  unsigned char expected[16] = {0x55};
  if (memcmp(&partial, expected, 16))
    return 4;
  CFRelease(a);
  return 0;
}

static int uf_dict_retains, uf_dict_releases;
static const void *uf_dict_retain(CFAllocatorRef allocator, const void *value) {
  (void)allocator;
  uf_dict_retains++;
  return value;
}
static void uf_dict_release(CFAllocatorRef allocator, const void *value) {
  (void)allocator;
  (void)value;
  uf_dict_releases++;
}

static CFHashCode uf_dict_collision_hash(const void *key) {
  (void)key;
  return 1;
}

static Boolean uf_dict_int_equal(const void *a, const void *b) {
  return *(const int *)a == *(const int *)b;
}

static CFComparisonResult uf_array_int_compare(const void *a, const void *b,
                                               void *context) {
  (*(int *)context)++;
  return (*(const int *)a > *(const int *)b) -
         (*(const int *)a < *(const int *)b);
}

int test_cfarray_custom_callbacks() {
  CFArrayCallBacks callbacks = {0, uf_dict_retain, uf_dict_release, NULL,
                                uf_dict_int_equal};
  int a = 42, b = 42, c = 17;
  uf_dict_retains = uf_dict_releases = 0;
  CFMutableArrayRef array = CFArrayCreateMutable(NULL, 0, &callbacks);
  CFArrayAppendValue(array, &a);
  CFArrayInsertValueAtIndex(array, 0, &c);
  CFArrayExchangeValuesAtIndices(array, 0, 1);
  if (CFArrayGetValueAtIndex(array, 0) != &a ||
      CFArrayGetValueAtIndex(array, 1) != &c || uf_dict_retains != 2 ||
      uf_dict_releases != 0)
    return 13;
  int comparisons = 0;
  CFArraySortValues(array, CFRangeMake(0, 2), uf_array_int_compare,
                    &comparisons);
  if (comparisons == 0 || CFArrayGetValueAtIndex(array, 0) != &c ||
      CFArrayGetValueAtIndex(array, 1) != &a || uf_dict_retains != 2 ||
      uf_dict_releases != 0)
    return 14;
  CFArrayExchangeValuesAtIndices(array, 1, 1);
  if (!CFArrayContainsValue(array, CFRangeMake(0, 2), &b) ||
      CFArrayGetFirstIndexOfValue(array, CFRangeMake(0, 2), &b) != 1 ||
      uf_dict_retains != 2)
    return 1;
  CFArraySetValueAtIndex(array, 0, &b);
  CFArrayRemoveValueAtIndex(array, 1);
  if (uf_dict_retains != 3 || uf_dict_releases != 2)
    return 2;
  CFArrayRemoveAllValues(array);
  CFArrayAppendValue(array, &a);
  CFRelease(array);
  if (uf_dict_retains != 4 || uf_dict_releases != 4)
    return 3;
  uf_dict_retains = uf_dict_releases = 0;
  array = CFArrayCreateMutable(NULL, 0, &callbacks);
  CFArrayAppendValue(array, &a);
  CFArrayRef copy = CFArrayCreateCopy(NULL, array);
  CFMutableArrayRef mutable_copy = CFArrayCreateMutableCopy(NULL, 0, copy);
  CFRelease(array);
  if (uf_dict_retains != 3 || uf_dict_releases != 1 ||
      !CFArrayContainsValue(copy, CFRangeMake(0, 1), &b))
    return 7;
  CFArraySetValueAtIndex(mutable_copy, 0, &c);
  CFRelease(copy);
  CFRelease(mutable_copy);
  if (uf_dict_retains != 4 || uf_dict_releases != 4)
    return 8;
  uf_dict_retains = uf_dict_releases = 0;
  array = CFArrayCreateMutable(NULL, 0, &callbacks);
  CFArrayAppendValue(array, &a);
  CFArrayAppendValue(array, &c);
  const void *replacement[] = {&a, &b};
  CFArrayReplaceValues(array, CFRangeMake(0, 2), replacement, 2);
  if (uf_dict_retains != 4 || uf_dict_releases != 2 ||
      CFArrayGetValueAtIndex(array, 0) != &a ||
      CFArrayGetValueAtIndex(array, 1) != &b)
    return 19;
  CFRelease(array);
  if (uf_dict_releases != 4)
    return 20;
  uf_dict_retains = uf_dict_releases = 0;
  array = CFArrayCreateMutable(NULL, 0, &callbacks);
  CFArrayAppendValue(array, &a);
  CFArrayAppendArray(array, array, CFRangeMake(0, 1));
  CFRelease(array);
  if (uf_dict_retains != 2 || uf_dict_releases != 2)
    return 12;
  uf_dict_retains = uf_dict_releases = 0;
  const void *initial[] = {&a, &c};
  copy = CFArrayCreate(NULL, initial, 2, &callbacks);
  if (uf_dict_retains != 2 || CFArrayGetCount(copy) != 2 ||
      !CFArrayContainsValue(copy, CFRangeMake(0, 2), &b))
    return 9;
  CFRelease(copy);
  if (uf_dict_releases != 2)
    return 10;
  copy = CFArrayCreate(NULL, NULL, 0, NULL);
  if (CFArrayGetCount(copy) != 0)
    return 11;
  CFRelease(copy);
  unsigned char bytes[16] = {0x91};
  CFUUIDRef uuid = uf_uuid_from_bytes(bytes, 0);
  array = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);
  CFArrayAppendValue(array, uuid);
  if (CFGetRetainCount(uuid) != 2 ||
      !CFArrayContainsValue(array, CFRangeMake(0, 1), uuid))
    return 4;
  CFRelease(array);
  if (CFGetRetainCount(uuid) != 1)
    return 5;
  CFRelease(uuid);
  int low = -100, high = 100;
  array = CFArrayCreateMutable(NULL, 0, NULL);
  CFArrayAppendValue(array, &low);
  CFArrayAppendValue(array, &a);
  CFArrayAppendValue(array, &b);
  CFArrayAppendValue(array, &c);
  CFArrayAppendValue(array, &high);
  comparisons = 0;
  CFArraySortValues(array, CFRangeMake(1, 3), uf_array_int_compare,
                    &comparisons);
  if (!comparisons || CFArrayGetValueAtIndex(array, 0) != &low ||
      CFArrayGetValueAtIndex(array, 1) != &c ||
      CFArrayGetValueAtIndex(array, 2) != &a ||
      CFArrayGetValueAtIndex(array, 3) != &b ||
      CFArrayGetValueAtIndex(array, 4) != &high)
    return 15;
  CFRelease(array);
  NSNumber *n1 = [[NSNumber alloc] initWithInt:42];
  NSNumber *n2 = [[NSNumber alloc] initWithInt:42];
  NSNumber *n3 = [[NSNumber alloc] initWithInt:17];
  array = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);
  CFArrayAppendValue(array, n1);
  int matches = CFArrayContainsValue(array, CFRangeMake(0, 1), n2) &&
                !CFArrayContainsValue(array, CFRangeMake(0, 1), n3);
  CFRelease(array);
  [n1 release];
  [n2 release];
  [n3 release];
  if (!matches)
    return 6;
  int many[64];
  array = CFArrayCreateMutable(NULL, 0, NULL);
  for (int i = 0; i < 64; i++) {
    many[i] = (i * 37) % 64;
    CFArrayAppendValue(array, &many[i]);
  }
  comparisons = 0;
  CFArraySortValues(array, CFRangeMake(0, 64), uf_array_int_compare,
                    &comparisons);
  for (int i = 0; i < 64; i++)
    if (*(const int *)CFArrayGetValueAtIndex(array, i) != i)
      return 16;
  if (comparisons <= 0 || comparisons > 1024)
    return 17;
  int needle = 33, below = -1, above = 100;
  comparisons = 0;
  if (CFArrayBSearchValues(array, CFRangeMake(2, 60), &needle,
                           uf_array_int_compare, &comparisons) != 33 ||
      CFArrayBSearchValues(array, CFRangeMake(2, 60), &below,
                           uf_array_int_compare, &comparisons) != 2 ||
      CFArrayBSearchValues(array, CFRangeMake(2, 60), &above,
                           uf_array_int_compare, &comparisons) != 62 ||
      CFArrayBSearchValues(array, CFRangeMake(7, 0), &needle,
                           uf_array_int_compare, &comparisons) != 7 ||
      comparisons == 0)
    return 18;
  CFRelease(array);
  return 0;
}

int test_cfurl_legacy_percent_decode() {
  CFStringRef original =
      CFStringCreateWithCString(NULL, "A%2FB%E4%B8%AD", kCFStringEncodingUTF8);
  CFStringRef leave =
      CFStringCreateWithCString(NULL, "/", kCFStringEncodingUTF8);
  CFStringRef result =
      CFURLCreateStringByReplacingPercentEscapes(NULL, original, leave);
  CFRelease(leave);
  char buffer[64];
  if (!result ||
      !CFStringGetCString(result, buffer, sizeof(buffer),
                          kCFStringEncodingUTF8) ||
      strcmp(buffer, "A%2FB\xe4\xb8\xad"))
    return 1;
  CFRelease(result);
  result = CFURLCreateStringByReplacingPercentEscapes(NULL, original, NULL);
  if (!result ||
      !CFStringGetCString(result, buffer, sizeof(buffer),
                          kCFStringEncodingUTF8) ||
      strcmp(buffer, "A%2FB%E4%B8%AD"))
    return 2;
  CFRelease(result);
  CFRelease(original);
  original =
      CFStringCreateWithCString(NULL, "%C3%A9%2F", kCFStringEncodingUTF8);
  leave = CFStringCreateWithCString(NULL, "\xc3\xa9/", kCFStringEncodingUTF8);
  result = CFURLCreateStringByReplacingPercentEscapes(NULL, original, leave);
  if (!result ||
      !CFStringGetCString(result, buffer, sizeof(buffer),
                          kCFStringEncodingUTF8) ||
      strcmp(buffer, "%C3%A9%2F"))
    return 4;
  CFRelease(result);
  CFRelease(leave);
  CFRelease(original);
  original = CFStringCreateWithCString(NULL, "%FF", kCFStringEncodingUTF8);
  leave = CFStringCreateWithCString(NULL, "", kCFStringEncodingUTF8);
  result = CFURLCreateStringByReplacingPercentEscapes(NULL, original, leave);
  CFRelease(leave);
  CFRelease(original);
  return result == NULL ? 0 : 3;
}

static int uf_url_string_equals(CFStringRef string, const char *expected) {
  char buffer[128];
  return string &&
         CFStringGetCString(string, buffer, sizeof(buffer),
                            kCFStringEncodingUTF8) &&
         !strcmp(buffer, expected);
}

int test_cfurl_query_and_fragment() {
  const char *inputs[] = {"http://e/a?q=%C3%A9%2F#f%20x",
                          "file://host/a?q=%2F#f%20x"};
  const char *raw_query[] = {"q=%C3%A9%2F", "q=%2F"};
  const char *decoded_query[] = {"q=\xc3\xa9%2F", "q=%2F"};
  CFStringRef leave =
      CFStringCreateWithCString(NULL, "/", kCFStringEncodingUTF8);
  for (int i = 0; i < 2; i++) {
    CFStringRef text =
        CFStringCreateWithCString(NULL, inputs[i], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
    CFRelease(text);
    CFStringRef raw = CFURLCopyQueryString(url, NULL);
    CFStringRef decoded = CFURLCopyQueryString(url, leave);
    CFStringRef fragment = CFURLCopyFragment(url, leave);
    CFRelease(url);
    if (!uf_url_string_equals(raw, raw_query[i]) ||
        !uf_url_string_equals(decoded, decoded_query[i]) ||
        !uf_url_string_equals(fragment, "f x"))
      return i + 1;
    CFRelease(raw);
    CFRelease(decoded);
    CFRelease(fragment);
  }
  CFRelease(leave);
  CFStringRef text =
      CFStringCreateWithCString(NULL, "http://e/a?#", kCFStringEncodingUTF8);
  CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
  CFRelease(text);
  CFStringRef query = CFURLCopyQueryString(url, NULL);
  CFStringRef fragment = CFURLCopyFragment(url, NULL);
  CFRelease(url);
  int ok =
      uf_url_string_equals(query, "") && uf_url_string_equals(fragment, "");
  CFRelease(query);
  CFRelease(fragment);
  return ok ? 0 : 3;
}

int test_cfurl_create_bytes() {
  const char *bytes = "http://example.com/a%2Fb";
  CFURLRef url = CFURLCreateWithBytes(NULL, (const UInt8 *)bytes, strlen(bytes),
                                      kCFStringEncodingASCII, NULL);
  if (!url)
    return 1;
  CFStringRef scheme = CFURLCopyScheme(url);
  CFStringRef path = CFURLCopyPath(url);
  Boolean is_file = [(NSURL *)url isFileURL];
  CFRelease(url);
  int ok = uf_url_string_equals(scheme, "http") &&
           uf_url_string_equals(path, "/a%2Fb") && !is_file;
  if (scheme)
    CFRelease(scheme);
  if (path)
    CFRelease(path);
  return ok ? 0 : 2;
}

int test_cfurl_directory_path() {
  const char *inputs[] = {"http://e",       "http://e/",    "http://e/a/",
                          "http://e/a",     "http://e/a/.", "http://e/a/..",
                          "http://e/a/%2e", "about:blank",  "abc/"};
  const Boolean expected[] = {false, true,  true,  false, true,
                              true,  false, false, true};
  for (int i = 0; i < 9; i++) {
    CFStringRef text =
        CFStringCreateWithCString(NULL, inputs[i], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
    CFRelease(text);
    Boolean actual = CFURLHasDirectoryPath(url);
    CFRelease(url);
    if (actual != expected[i])
      return i + 1;
  }
  CFStringRef path =
      CFStringCreateWithCString(NULL, "/tmp/test", kCFStringEncodingUTF8);
  CFURLRef url =
      CFURLCreateWithFileSystemPath(NULL, path, kCFURLPOSIXPathStyle, true);
  CFRelease(path);
  Boolean actual = CFURLHasDirectoryPath(url);
  CFRelease(url);
  if (!actual)
    return 10;
  path = CFStringCreateWithCString(NULL, "//", kCFStringEncodingUTF8);
  url = CFURLCreateWithFileSystemPath(NULL, path, kCFURLPOSIXPathStyle, false);
  CFRelease(path);
  actual = CFURLHasDirectoryPath(url);
  CFRelease(url);
  return actual ? 11 : 0;
}

int test_cfurl_escaped_path() {
  const char *inputs[] = {"http://host/a%2Fb%20c", "file://host/a%2Fb%20c",
                          "relative/path",         "http://e",
                          "about:blank",           "http://e/"};
  const char *expected[] = {"/a%2Fb%20c", "/a%2Fb%20c", "relative/path",
                            "",           NULL,         "/"};
  const char *strict_expected[] = {"a%2Fb%20c", "a%2Fb%20c", "relative/path",
                                   NULL,        NULL,        ""};
  for (int i = 0; i < 6; i++) {
    CFStringRef text =
        CFStringCreateWithCString(NULL, inputs[i], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
    CFRelease(text);
    CFStringRef path = CFURLCopyPath(url);
    Boolean absolute = 77;
    CFStringRef strict = CFURLCopyStrictPath(url, &absolute);
    Boolean decomposable = CFURLCanBeDecomposed(url);
    CFRelease(url);
    if (decomposable != (i != 4) || absolute != (i < 2 || i == 5) ||
        (strict_expected[i] &&
         !uf_url_string_equals(strict, strict_expected[i])) ||
        (!strict_expected[i] && strict))
      return i + 10;
    if (strict)
      CFRelease(strict);
    if (expected[i]) {
      if (!uf_url_string_equals(path, expected[i]))
        return i + 1;
      CFRelease(path);
    } else if (path) {
      CFRelease(path);
      return i + 1;
    }
  }
  CFStringRef text =
      CFStringCreateWithCString(NULL, "/tmp/a b#c", kCFStringEncodingUTF8);
  CFURLRef url =
      CFURLCreateWithFileSystemPath(NULL, text, kCFURLPOSIXPathStyle, true);
  CFRelease(text);
  CFStringRef path = CFURLCopyPath(url);
  Boolean absolute = 0;
  CFStringRef strict = CFURLCopyStrictPath(url, &absolute);
  CFRelease(url);
  int ok = uf_url_string_equals(path, "/tmp/a%20b%23c/") && absolute &&
           uf_url_string_equals(strict, "tmp/a%20b%23c/");
  CFRelease(path);
  CFRelease(strict);
  return ok ? 0 : 6;
}

int test_cfurl_host_name() {
  const char *inputs[] = {"file:///tmp/a",
                          "file://localhost/tmp/a",
                          "file://server:8/tmp",
                          "http://e%78ample.com:3/a",
                          "http://user:pass@[::1]:5/",
                          "http:/p"};
  const char *expected[] = {NULL,          "localhost", "server",
                            "example.com", "::1",       NULL};
  for (int i = 0; i < 6; i++) {
    CFStringRef text =
        CFStringCreateWithCString(NULL, inputs[i], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
    CFRelease(text);
    CFStringRef host = CFURLCopyHostName(url);
    CFRelease(url);
    if (expected[i]) {
      char buffer[64];
      if (!host ||
          !CFStringGetCString(host, buffer, sizeof(buffer),
                              kCFStringEncodingUTF8) ||
          strcmp(buffer, expected[i]))
        return i + 1;
      CFRelease(host);
    } else if (host) {
      CFRelease(host);
      return i + 1;
    }
  }
  return 0;
}

int test_cfurl_port_number() {
  const char *inputs[] = {"http://example.com:0/",  "http://e:65536/",
                          "http://u:p@[::1]:8080/", "http://example.com:/",
                          "http://e:abc/",          "http://e:999999999999/",
                          "file://server:8080/tmp", "http://e:-12/"};
  const int expected[] = {0, 65536, 8080, -1, -1, 2147483647, 8080, -12};
  for (int i = 0; i < 8; i++) {
    CFStringRef text =
        CFStringCreateWithCString(NULL, inputs[i], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithString(NULL, text, NULL);
    CFRelease(text);
    if (!url || CFURLGetPortNumber(url) != expected[i])
      return i + 1;
    CFRelease(url);
  }
  return 0;
}

int test_cfurl_copy_scheme() {
  CFStringRef url_text = CFStringCreateWithCString(NULL, "HTTP://example.com/a",
                                                   kCFStringEncodingUTF8);
  CFURLRef url = CFURLCreateWithString(NULL, url_text, NULL);
  CFRelease(url_text);
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CFStringRef scheme = CFURLCopyScheme(url);
  [pool drain];
  CFRelease(url);
  char buffer[16];
  if (!scheme ||
      !CFStringGetCString(scheme, buffer, sizeof(buffer),
                          kCFStringEncodingUTF8) ||
      strcmp(buffer, "HTTP"))
    return 1;
  CFRelease(scheme);
  url_text =
      CFStringCreateWithCString(NULL, "relative/path", kCFStringEncodingUTF8);
  url = CFURLCreateWithString(NULL, url_text, NULL);
  CFRelease(url_text);
  scheme = CFURLCopyScheme(url);
  CFRelease(url);
  if (scheme)
    return 2;
  url_text =
      CFStringCreateWithCString(NULL, "/tmp/file", kCFStringEncodingUTF8);
  url = CFURLCreateWithFileSystemPath(NULL, url_text, kCFURLPOSIXPathStyle,
                                      false);
  CFRelease(url_text);
  scheme = CFURLCopyScheme(url);
  CFRelease(url);
  int ok = scheme &&
           CFStringGetCString(scheme, buffer, sizeof(buffer),
                              kCFStringEncodingUTF8) &&
           !strcmp(buffer, "file");
  if (scheme)
    CFRelease(scheme);
  return ok ? 0 : 3;
}

int test_cflocale_copy_lifetime() {
  CFStringRef identifier =
      CFStringCreateWithCString(NULL, "en_US", kCFStringEncodingUTF8);
  CFLocaleRef locale = CFLocaleCreate(NULL, identifier);
  CFRelease(identifier);
  CFLocaleRef copy = CFLocaleCreateCopy(kCFAllocatorSystemDefault, locale);
  CFRelease(locale);
  CFStringRef country = CFLocaleGetValue(copy, kCFLocaleCountryCode);
  char buffer[16];
  int ok = country &&
           CFStringGetCString(country, buffer, sizeof(buffer),
                              kCFStringEncodingUTF8) &&
           !strcmp(buffer, "US");
  CFRelease(copy);
  return ok ? 0 : 1;
}

int test_cfdata_copy_and_empty_range() {
  unsigned char source[] = {3, 4, 5};
  NSMutableData *mutable_data = [[NSMutableData alloc] initWithBytes:source
                                                              length:3];
  CFDataRef copy = CFDataCreateCopy(NULL, (CFDataRef)mutable_data);
  ((unsigned char *)[mutable_data mutableBytes])[0] = 9;
  unsigned char result[] = {0xaa, 0xbb, 0xcc};
  CFDataGetBytes(copy, CFRangeMake(3, 0), NULL);
  CFDataGetBytes(copy, CFRangeMake(1, 1), &result[1]);
  int ok = CFDataGetLength(copy) == 3 && CFDataGetBytePtr(copy)[0] == 3 &&
           result[0] == 0xaa && result[1] == 4 && result[2] == 0xcc;
  CFRelease(copy);
  [mutable_data release];
  copy = CFDataCreate(NULL, NULL, 0);
  CFDataGetBytes(copy, CFRangeMake(0, 0), NULL);
  ok = ok && CFDataGetLength(copy) == 0;
  CFRelease(copy);
  unsigned char payload[64];
  for (int i = 0; i < 64; i++)
    payload[i] = (unsigned char)(i * 3);
  CFMutableDataRef growing = CFDataCreateMutable(NULL, 0);
  CFDataAppendBytes(growing, payload, 64);
  CFDataAppendBytes(growing, CFDataGetBytePtr(growing) + 8, 48);
  CFDataAppendBytes(growing, NULL, 0);
  const unsigned char *bytes = CFDataGetBytePtr(growing);
  ok = ok && CFDataGetLength(growing) == 112 &&
       memcmp(bytes, payload, 64) == 0 &&
       memcmp(bytes + 64, payload + 8, 48) == 0;
  CFRelease(growing);
  return ok ? 0 : 1;
}

int test_cfdata_no_copy_deallocator() {
  unsigned char *bytes = malloc(32);
  if (!bytes)
    return 1;
  bytes[0] = 0x5a;
  CFDataRef data = CFDataCreateWithBytesNoCopy(NULL, bytes, 32, NULL);
  if (CFDataGetBytePtr(data) != bytes || CFDataGetBytePtr(data)[0] != 0x5a)
    return 2;
  CFRelease(data);
  if (malloc_zone_from_ptr(bytes) != NULL)
    return 3;
  bytes = malloc(32);
  if (!bytes)
    return 4;
  bytes[0] = 0x7b;
  data = CFDataCreateWithBytesNoCopy(NULL, bytes, 32, kCFAllocatorNull);
  if (CFDataGetBytePtr(data) != bytes)
    return 5;
  CFRelease(data);
  if (malloc_zone_from_ptr(bytes) == NULL || bytes[0] != 0x7b)
    return 6;
  free(bytes);
  return 0;
}

int test_cfdata_equality_and_dictionary_keys() {
  unsigned char bytes[] = {4, 5, 6};
  unsigned char other_bytes[] = {4, 5, 7};
  CFDataRef a = CFDataCreate(NULL, bytes, 3);
  CFDataRef b = CFDataCreate(NULL, bytes, 3);
  CFDataRef different = CFDataCreate(NULL, other_bytes, 3);
  CFDataRef empty_a = CFDataCreate(NULL, NULL, 0);
  CFDataRef empty_b = CFDataCreate(NULL, NULL, 0);
  CFStringRef string =
      CFStringCreateWithCString(NULL, "data", kCFStringEncodingUTF8);
  if (!CFEqual(a, b) || CFEqual(a, different) || !CFEqual(empty_a, empty_b) ||
      CFEqual(a, string) || CFEqual(string, a) || CFHash(a) != CFHash(b))
    return 1;
  CFMutableDictionaryRef dict =
      CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks,
                                &kCFTypeDictionaryValueCallBacks);
  CFDictionarySetValue(dict, a, string);
  if (CFDictionaryGetValue(dict, b) != string ||
      CFDictionaryContainsKey(dict, different))
    return 2;
  CFRelease(dict);
  CFRelease(a);
  CFRelease(b);
  CFRelease(different);
  CFRelease(empty_a);
  CFRelease(empty_b);
  CFRelease(string);
  return 0;
}

int test_cfdata_mutable_copy_and_length() {
  unsigned char original[] = {1, 2, 3};
  CFDataRef source = CFDataCreate(NULL, original, 3);
  CFMutableDataRef data = CFDataCreateMutableCopy(NULL, 0, source);
  CFDataSetLength(data, 6);
  UInt8 *bytes = CFDataGetMutableBytePtr(data);
  if (CFDataGetLength(data) != 6 || memcmp(bytes, original, 3) != 0 ||
      bytes[3] || bytes[4] || bytes[5])
    return 1;
  bytes[0] = 9;
  if (CFDataGetBytePtr(source)[0] != 1)
    return 2;
  CFDataSetLength(data, 1);
  CFDataIncreaseLength(data, 3);
  bytes = CFDataGetMutableBytePtr(data);
  if (bytes[0] != 9 || bytes[1] || bytes[2] || bytes[3])
    return 3;
  CFDataSetLength(data, 0);
  (void)CFDataGetMutableBytePtr(data);
  CFRelease(data);
  CFRelease(source);
  return 0;
}

int test_cfdata_replace_and_delete() {
  const unsigned char original[] = {0, 1, 2, 3, 4, 5};
  CFMutableDataRef data = CFDataCreateMutable(NULL, 0);
  CFDataAppendBytes(data, original, 6);
  CFDataReplaceBytes(data, CFRangeMake(1, 2), CFDataGetBytePtr(data) + 3, 3);
  const unsigned char grown[] = {0, 3, 4, 5, 3, 4, 5};
  if (CFDataGetLength(data) != 7 ||
      memcmp(CFDataGetBytePtr(data), grown, sizeof(grown)))
    return 1;
  CFDataDeleteBytes(data, CFRangeMake(2, 3));
  const unsigned char shrunk[] = {0, 3, 4, 5};
  if (CFDataGetLength(data) != 4 ||
      memcmp(CFDataGetBytePtr(data), shrunk, sizeof(shrunk)))
    return 2;
  const unsigned char prefix[] = {9, 8};
  CFDataReplaceBytes(data, CFRangeMake(0, 0), prefix, 2);
  const unsigned char inserted[] = {9, 8, 0, 3, 4, 5};
  if (CFDataGetLength(data) != 6 ||
      memcmp(CFDataGetBytePtr(data), inserted, sizeof(inserted)))
    return 3;
  CFDataReplaceBytes(data, CFRangeMake(1, 4), CFDataGetBytePtr(data) + 3, 2);
  const unsigned char overlap_shrunk[] = {9, 3, 4, 5};
  if (CFDataGetLength(data) != 4 ||
      memcmp(CFDataGetBytePtr(data), overlap_shrunk, sizeof(overlap_shrunk)))
    return 5;
  CFDataReplaceBytes(data, CFRangeMake(0, 4), NULL, 0);
  if (CFDataGetLength(data) != 0)
    return 4;
  CFRelease(data);
  return 0;
}

int test_cfdictionary_contains_value_callbacks() {
  CFDictionaryValueCallBacks values = {0, NULL, NULL, NULL, uf_dict_int_equal};
  int key, key2, missing, stored = 42, equal = 42, different = 17;
  CFMutableDictionaryRef dict =
      CFDictionaryCreateMutable(NULL, 0, NULL, &values);
  CFDictionarySetValue(dict, &key, &stored);
  CFDictionarySetValue(dict, &key2, &equal);
  if (!CFDictionaryContainsValue(dict, &equal) ||
      CFDictionaryContainsValue(dict, &different) ||
      CFDictionaryGetCountOfValue(dict, &equal) != 2 ||
      CFDictionaryGetCountOfValue(dict, &different) != 0 ||
      CFDictionaryGetCountOfKey(dict, &key) != 1 ||
      CFDictionaryGetCountOfKey(dict, &missing) != 0)
    return 1;
  CFRelease(dict);
  dict = CFDictionaryCreateMutable(NULL, 0, NULL, NULL);
  CFDictionarySetValue(dict, &key, NULL);
  if (!CFDictionaryContainsValue(dict, NULL) ||
      CFDictionaryGetCountOfValue(dict, NULL) != 1 ||
      CFDictionaryContainsValue(dict, &equal))
    return 2;
  CFRelease(dict);
  return 0;
}

int test_nsmutabledictionary_replacement_key_lifetime() {
  NSString *key = [[NSString alloc] initWithUTF8String:"key"];
  NSString *value = [[NSString alloc] initWithUTF8String:"value"];
  NSMutableDictionary *dict = [[NSMutableDictionary alloc] init];
  [dict setObject:value forKey:key];
  NSUInteger count = [key retainCount];
  for (int i = 0; i < 32; i++)
    [dict setObject:value forKey:key];
  int ok = [dict count] == 1 && [key retainCount] == count;
  [dict release];
  ok = ok && [key retainCount] == 1;
  [value release];
  [key release];
  return ok ? 0 : 1;
}

int test_nsmutablearray_remove_multiple_matches() {
  NSNumber *one = [NSNumber numberWithInt:1];
  NSNumber *two = [NSNumber numberWithInt:2];
  NSMutableArray *array =
      [[NSMutableArray alloc] initWithObjects:one, two, one, one, two, nil];
  [array removeObject:one];
  int ok = [array count] == 2 && [[array objectAtIndex:0] isEqual:two] &&
           [[array objectAtIndex:1] isEqual:two];
  [array release];
  NSString *value = [[NSString alloc] initWithUTF8String:"kept"];
  array = [[NSMutableArray alloc] initWithObjects:value, nil];
  [value release];
  const void *replacement[] = {[array objectAtIndex:0]};
  CFArrayReplaceValues((CFMutableArrayRef)array, CFRangeMake(0, 1), replacement,
                       1);
  ok = ok && [array count] == 1 &&
       !strcmp([[array objectAtIndex:0] UTF8String], "kept");
  [array release];
  return ok ? 0 : 1;
}

struct uf_array_visit_context {
  const void *values[3];
  int count;
};
static void uf_array_visit(const void *value, void *context) {
  struct uf_array_visit_context *visit = context;
  if (visit->count < 3)
    visit->values[visit->count] = value;
  visit->count++;
}

int test_cfarray_nonretaining_mutations() {
  int first, second, third;
  CFMutableArrayRef array = CFArrayCreateMutable(NULL, 0, NULL);
  CFArrayAppendValue(array, &first);
  CFArrayInsertValueAtIndex(array, 0, &second);
  CFArraySetValueAtIndex(array, 1, &third);
  CFArraySetValueAtIndex(array, 2, &first);
  if (CFArrayGetCount(array) != 3 ||
      CFArrayGetValueAtIndex(array, 0) != &second ||
      CFArrayGetValueAtIndex(array, 1) != &third ||
      CFArrayGetValueAtIndex(array, 2) != &first)
    return 1;
  CFArrayAppendValue(array, &third);
  CFArrayAppendValue(array, NULL);
  if (!CFArrayContainsValue(array, CFRangeMake(0, 5), &third) ||
      CFArrayGetFirstIndexOfValue(array, CFRangeMake(0, 5), &third) != 1 ||
      CFArrayGetLastIndexOfValue(array, CFRangeMake(0, 5), &third) != 3 ||
      CFArrayGetCountOfValue(array, CFRangeMake(0, 5), &third) != 2 ||
      CFArrayGetFirstIndexOfValue(array, CFRangeMake(0, 5), NULL) != 4 ||
      CFArrayContainsValue(array, CFRangeMake(0, 1), &third))
    return 4;
  const void *output[4] = {&second, NULL, NULL, &second};
  CFArrayGetValues(array, CFRangeMake(1, 2), &output[1]);
  if (output[0] != &second || output[1] != &third || output[2] != &first ||
      output[3] != &second)
    return 3;
  CFArrayGetValues(array, CFRangeMake(5, 0), NULL);
  struct uf_array_visit_context visit = {{NULL, NULL, NULL}, 0};
  CFArrayApplyFunction(array, CFRangeMake(1, 3), uf_array_visit, &visit);
  CFArrayApplyFunction(array, CFRangeMake(5, 0), uf_array_visit, &visit);
  if (visit.count != 3 || visit.values[0] != &third ||
      visit.values[1] != &first || visit.values[2] != &third)
    return 7;
  CFArrayAppendArray(array, array, CFRangeMake(0, 2));
  if (CFArrayGetCount(array) != 7 ||
      CFArrayGetValueAtIndex(array, 5) != &second ||
      CFArrayGetValueAtIndex(array, 6) != &third)
    return 8;
  const void *replacement[] = {NULL, &first};
  CFArrayReplaceValues(array, CFRangeMake(5, 2), replacement, 2);
  if (CFArrayGetValueAtIndex(array, 5) != NULL ||
      CFArrayGetValueAtIndex(array, 6) != &first)
    return 9;
  CFArrayRef copy = CFArrayCreateCopy(NULL, array);
  CFMutableArrayRef mutable_copy = CFArrayCreateMutableCopy(NULL, 0, copy);
  if (CFArrayGetValueAtIndex(copy, 1) != &third ||
      CFArrayGetValueAtIndex(mutable_copy, 2) != &first)
    return 6;
  CFArraySetValueAtIndex(mutable_copy, 0, &first);
  CFRelease(copy);
  CFRelease(mutable_copy);
  CFArrayRemoveAllValues(array);
  if (CFArrayGetCount(array) != 0)
    return 2;
  CFArrayAppendValue(array, &third);
  CFRelease(array);
  NSString *a = [[NSString alloc] initWithUTF8String:"same"];
  NSString *b = [[NSString alloc] initWithUTF8String:"same"];
  NSArray *bridged = [[NSArray alloc] initWithObjects:a, nil];
  int equal = CFArrayContainsValue((CFArrayRef)bridged, CFRangeMake(0, 1), b);
  [bridged release];
  [a release];
  [b release];
  if (!equal)
    return 5;
  return 0;
}

int test_cfdictionary_presence_and_collision() {
  CFDictionaryKeyCallBacks keys = {0,    NULL, NULL,
                                   NULL, NULL, uf_dict_collision_hash};
  int k1, k2, missing, value, sentinel;
  CFMutableDictionaryRef dict = CFDictionaryCreateMutable(NULL, 0, &keys, NULL);
  CFDictionarySetValue(dict, &k1, NULL);
  CFDictionarySetValue(dict, &k2, &value);
  CFDictionaryAddValue(dict, &k1, &value);
  const void *result = &sentinel;
  if (!CFDictionaryContainsKey(dict, &k1) ||
      !CFDictionaryGetValueIfPresent(dict, &k1, &result) || result != NULL ||
      CFDictionaryGetCount(dict) != 2)
    return 1;
  if (!CFDictionaryGetValueIfPresent(dict, &k2, &result) || result != &value ||
      !CFDictionaryGetValueIfPresent(dict, &k2, NULL))
    return 2;
  result = &sentinel;
  if (CFDictionaryContainsKey(dict, &missing) ||
      CFDictionaryGetValueIfPresent(dict, &missing, &result) ||
      result != &sentinel)
    return 3;
  CFDictionaryReplaceValue(dict, &missing, &sentinel);
  CFDictionaryReplaceValue(dict, &k1, &sentinel);
  if (CFDictionaryGetCount(dict) != 2 ||
      !CFDictionaryGetValueIfPresent(dict, &k1, &result) ||
      result != &sentinel || CFDictionaryContainsKey(dict, &missing))
    return 6;
  CFDictionaryRemoveValue(dict, &k1);
  if (CFDictionaryContainsKey(dict, &k1) || !CFDictionaryContainsKey(dict, &k2))
    return 4;
  CFRelease(dict);
  NSString *key = [[NSString alloc] initWithUTF8String:"key"];
  NSString *object = [[NSString alloc] initWithUTF8String:"value"];
  NSDictionary *bridged =
      [[NSDictionary alloc] initWithObjectsAndKeys:object, key, nil];
  result = NULL;
  if (!CFDictionaryGetValueIfPresent((CFDictionaryRef)bridged, key, &result) ||
      result != object ||
      !CFDictionaryContainsKey((CFDictionaryRef)bridged, key) ||
      !CFDictionaryContainsValue((CFDictionaryRef)bridged, object) ||
      CFDictionaryContainsValue((CFDictionaryRef)bridged, key))
    return 5;
  [bridged release];
  [object release];
  [key release];
  return 0;
}

int test_cfdictionary_callback_lifecycle() {
  CFDictionaryKeyCallBacks keys = {0,    uf_dict_retain, uf_dict_release,
                                   NULL, NULL,           NULL};
  CFDictionaryValueCallBacks values = {0, uf_dict_retain, uf_dict_release, NULL,
                                       NULL};
  int k1, k2, v1, v2;
  uf_dict_retains = uf_dict_releases = 0;
  CFMutableDictionaryRef dict = CFDictionaryCreateMutable(NULL, 0, &keys, NULL);
  CFDictionarySetValue(dict, &k1, &v1);
  CFDictionarySetValue(dict, &k2, &v2);
  CFDictionaryRemoveValue(dict, &k1);
  if (uf_dict_retains != 2 || uf_dict_releases != 1)
    return 1;
  CFRelease(dict);
  if (uf_dict_releases != 2)
    return 2;
  uf_dict_retains = uf_dict_releases = 0;
  dict = CFDictionaryCreateMutable(NULL, 0, NULL, &values);
  CFDictionarySetValue(dict, &k1, &v1);
  CFDictionarySetValue(dict, &k2, &v2);
  CFDictionaryRemoveAllValues(dict);
  if (uf_dict_retains != 2 || uf_dict_releases != 2 ||
      CFDictionaryGetCount(dict) != 0)
    return 3;
  CFDictionarySetValue(dict, &k1, &v1);
  CFRelease(dict);
  return uf_dict_retains == 3 && uf_dict_releases == 3 ? 0 : 4;
}

int test_cfdictionary_replaces_equal_key() {
  CFDictionaryKeyCallBacks keys = {
      0,    uf_dict_retain,    uf_dict_release,
      NULL, uf_dict_int_equal, uf_dict_collision_hash};
  CFDictionaryValueCallBacks values = {0, uf_dict_retain, uf_dict_release, NULL,
                                       NULL};
  int original = 5, equal_key = 5, first, second;
  uf_dict_retains = uf_dict_releases = 0;
  CFMutableDictionaryRef dict =
      CFDictionaryCreateMutable(NULL, 0, &keys, &values);
  CFDictionarySetValue(dict, &original, &first);
  CFDictionarySetValue(dict, &equal_key, &second);
  const void *stored_key = NULL;
  CFDictionaryGetKeysAndValues(dict, &stored_key, NULL);
  if (CFDictionaryGetCount(dict) != 1 || stored_key != &equal_key ||
      CFDictionaryGetValue(dict, &equal_key) != &second ||
      uf_dict_retains != 4 || uf_dict_releases != 2)
    return 1;
  CFRelease(dict);
  return uf_dict_retains == 4 && uf_dict_releases == 4 ? 0 : 2;
}

struct uf_dictionary_visit_context {
  const void *key1;
  const void *key2;
  const void *value1;
  int seen;
};
static void uf_dictionary_visit(const void *key, const void *value, void *ctx) {
  struct uf_dictionary_visit_context *visit = ctx;
  if (key == visit->key1 && value == visit->value1)
    visit->seen |= 1;
  else if (key == visit->key2 && value == NULL)
    visit->seen |= 2;
  else
    visit->seen |= 4;
}

int test_cfdictionary_apply_function() {
  int key1, key2, value1;
  const void *keys[] = {&key1, &key2};
  const void *values[] = {&value1, NULL};
  CFDictionaryRef dict = CFDictionaryCreate(NULL, keys, values, 2, NULL, NULL);
  struct uf_dictionary_visit_context visit = {&key1, &key2, &value1, 0};
  CFDictionaryApplyFunction(dict, uf_dictionary_visit, &visit);
  CFRelease(dict);
  if (visit.seen != 3)
    return 1;
  dict = CFDictionaryCreate(NULL, NULL, NULL, 0, NULL, NULL);
  visit.seen = 0;
  CFDictionaryApplyFunction(dict, uf_dictionary_visit, &visit);
  CFRelease(dict);
  if (visit.seen != 0)
    return 2;
  NSString *key = [[NSString alloc] initWithUTF8String:"key"];
  NSString *value = [[NSString alloc] initWithUTF8String:"value"];
  NSDictionary *bridged =
      [[NSDictionary alloc] initWithObjectsAndKeys:value, key, nil];
  visit.key1 = key;
  visit.value1 = value;
  visit.seen = 0;
  CFDictionaryApplyFunction((CFDictionaryRef)bridged, uf_dictionary_visit,
                            &visit);
  [bridged release];
  [value release];
  [key release];
  return visit.seen == 1 ? 0 : 3;
}

int test_cfdictionary_create_and_copy_callbacks() {
  CFDictionaryKeyCallBacks keys = {0,    uf_dict_retain, uf_dict_release,
                                   NULL, NULL,           NULL};
  CFDictionaryValueCallBacks values = {0, uf_dict_retain, uf_dict_release, NULL,
                                       NULL};
  int k1, k2, v1, v2, v3;
  const void *source_keys[] = {&k1, &k2};
  const void *source_values[] = {&v1, &v2};
  uf_dict_retains = uf_dict_releases = 0;
  CFDictionaryRef source =
      CFDictionaryCreate(NULL, source_keys, source_values, 2, &keys, &values);
  CFDictionaryRef copy = CFDictionaryCreateCopy(NULL, source);
  CFMutableDictionaryRef mutable_copy =
      CFDictionaryCreateMutableCopy(NULL, 0, copy);
  CFRelease(source);
  if (uf_dict_retains != 12 || uf_dict_releases != 4 ||
      CFDictionaryGetCount(copy) != 2 || CFDictionaryGetValue(copy, &k2) != &v2)
    return 1;
  CFDictionarySetValue(mutable_copy, &k1, &v3);
  if (CFDictionaryGetValue(copy, &k1) != &v1 ||
      CFDictionaryGetValue(mutable_copy, &k1) != &v3)
    return 2;
  CFRelease(copy);
  CFRelease(mutable_copy);
  if (uf_dict_retains != 14 || uf_dict_releases != 14)
    return 3;
  const void *null_values[] = {NULL};
  source = CFDictionaryCreate(NULL, source_keys, null_values, 1, NULL, NULL);
  if (!CFDictionaryContainsKey(source, &k1) ||
      CFDictionaryGetValue(source, &k1) != NULL)
    return 4;
  CFRelease(source);
  return 0;
}

int test_cfuuid_cf_collection_interop() {
  unsigned char bytes[16] = {0x33};
  CFUUIDRef a = uf_uuid_from_bytes(bytes, 0), b = uf_uuid_from_bytes(bytes, 0);
  bytes[15] = 1;
  CFUUIDRef other = uf_uuid_from_bytes(bytes, 0);
  CFStringRef value =
      CFStringCreateWithCString(NULL, "value", kCFStringEncodingUTF8);
  if (!CFEqual(a, b) || CFEqual(a, other) || CFEqual(a, value) ||
      CFEqual(value, a))
    return 1;
  CFMutableDictionaryRef dictionary =
      CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks,
                                &kCFTypeDictionaryValueCallBacks);
  CFDictionarySetValue(dictionary, a, value);
  CFRelease(a);
  if (CFDictionaryGetValue(dictionary, b) != value ||
      CFDictionaryGetValue(dictionary, other))
    return 2;
  CFRelease(dictionary);
  CFRelease(b);
  CFRelease(other);
  CFRelease(value);
  return 0;
}

int test_cfstring_character_creation() {
  UniChar units[] = {'A', 0, 'B', 0xd800};
  id pool = [[NSAutoreleasePool alloc] init];
  CFStringRef string =
      CFStringCreateWithCharacters(kCFAllocatorSystemDefault, units, 4);
  units[0] = 'X';
  [pool release];
  if (!string || CFStringGetLength(string) != 4 ||
      CFStringGetCharacterAtIndex(string, 0) != 'A' ||
      CFStringGetCharacterAtIndex(string, 1) ||
      CFStringGetCharacterAtIndex(string, 2) != 'B' ||
      CFStringGetCharacterAtIndex(string, 3) != 0xd800)
    return 1;
  CFRelease(string);
  pool = [[NSAutoreleasePool alloc] init];
  string = CFStringCreateWithCharacters(NULL, NULL, 0);
  [pool release];
  if (!string || CFStringGetLength(string))
    return 2;
  CFRelease(string);
  return 0;
}

int test_vsnprintf() {
  char *str;

  // Test %s
  str = str_format("%s", "test");
  if (strcmp(str, "test") != 0) {
    free(str);
    return -1;
  }
  free(str);
  // Test %s NULL
  str = str_format("%s", NULL);
  if (strcmp(str, "(null)") != 0) {
    free(str);
    return -2;
  }
  free(str);
  // Test % without a specifier
  str = str_format("abc%");
  if (strcmp(str, "abc") != 0) {
    free(str);
    return -3;
  }
  free(str);
  // Test %x
  str = str_format("%x", 2042);
  if (strcmp(str, "7fa") != 0) {
    free(str);
    return -4;
  }
  free(str);
  str = str_format("0x%08x", 184638698);
  if (strcmp(str, "0x0b015cea") != 0) {
    free(str);
    return -5;
  }
  free(str);
  // Test %d
  str = str_format("%d|%8d|%08d|%.d|%8.d|%.3d|%8.3d|%08.3d|%*d|%0*d", 5, 5, 5,
                   5, 5, 5, 5, 5, 8, 5, 8, 5);
  if (strcmp(str, "5|       5|00000005|5|       5|005|     005|     005|       "
                  "5|00000005") != 0) {
    free(str);
    return -6;
  }
  free(str);
  // Test %d with alternative form
  str = str_format("%#.2d", 5);
  if (strcmp(str, "05") != 0) {
    free(str);
    return -7;
  }
  free(str);
  // Test %f
  str = str_format("%f|%8f|%08f|%.f|%8.f|%.3f|%8.3f|%08.3f|%*f|%0*f", 10.12345,
                   10.12345, 10.12345, 10.12345, 10.12345, 10.12345, 10.12345,
                   10.12345, 8, 10.12345, 8, 10.12345);
  if (strcmp(str, "10.123450|10.123450|10.123450|10|      10|10.123|  "
                  "10.123|0010.123|10.123450|10.123450") != 0) {
    free(str);
    return -8;
  }
  free(str);
  str = str_format("%f|%8f|%08f|%.f|%8.f|%.3f|%8.3f|%08.3f|%*f|%0*f", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-10.123450|-10.123450|-10.123450|-10|     -10|-10.123| "
                  "-10.123|-010.123|-10.123450|-10.123450") != 0) {
    free(str);
    return -9;
  }
  free(str);
  // Test %e
  str = str_format("%e|%8e|%08e|%.e|%8.e|%.3e|%8.3e|%08.3e|%*e|%0*e", 10.12345,
                   10.12345, 10.12345, 10.12345, 10.12345, 10.12345, 10.12345,
                   10.12345, 8, 10.12345, 8, 10.12345);
  if (strcmp(str,
             "1.012345e+01|1.012345e+01|1.012345e+01|1e+01|   "
             "1e+01|1.012e+01|1.012e+01|1.012e+01|1.012345e+01|1.012345e+01") !=
      0) {
    free(str);
    return -10;
  }
  free(str);
  str = str_format("%e|%8e|%08e|%.e|%8.e|%.3e|%8.3e|%08.3e|%*e|%0*e", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-1.012345e+01|-1.012345e+01|-1.012345e+01|-1e+01|  "
                  "-1e+01|-1.012e+01|-1.012e+01|-1.012e+01|-1.012345e+01|-1."
                  "012345e+01") != 0) {
    free(str);
    return -11;
  }
  free(str);
  // Test %g
  str = str_format("%g|%8g|%08g|%.g|%8.g|%.3g|%8.3g|%08.3g|%*g|%0*g", 10.12345,
                   10.12345, 10.12345, 10.12345, 10.12345, 10.12345, 10.12345,
                   10.12345, 8, 10.12345, 8, 10.12345);
  if (strcmp(str, "10.1235| 10.1235|010.1235|1e+01|   1e+01|10.1|    "
                  "10.1|000010.1| 10.1235|010.1235") != 0) {
    free(str);
    return -12;
  }
  free(str);
  str = str_format("%g|%8g|%08g|%.g|%8.g|%.3g|%8.3g|%08.3g|%*g|%0*g", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-10.1235|-10.1235|-10.1235|-1e+01|  -1e+01|-10.1|   "
                  "-10.1|-00010.1|-10.1235|-10.1235") != 0) {
    free(str);
    return -13;
  }
  free(str);
  str = str_format("%f|%8f|%08f|%.f|%8.f|%.3f|%8.3f|%08.3f|%*f|%0*f", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-10.123450|-10.123450|-10.123450|-10|     -10|-10.123| "
                  "-10.123|-010.123|-10.123450|-10.123450") != 0) {
    free(str);
    return -14;
  }
  free(str);
  // Test %e
  str = str_format("%e|%8e|%08e|%.e|%8.e|%.3e|%8.3e|%08.3e|%*e|%0*e", 10.12345,
                   10.12345, 10.12345, 10.12345, 10.12345, 10.12345, 10.12345,
                   10.12345, 8, 10.12345, 8, 10.12345);
  if (strcmp(str,
             "1.012345e+01|1.012345e+01|1.012345e+01|1e+01|   "
             "1e+01|1.012e+01|1.012e+01|1.012e+01|1.012345e+01|1.012345e+01") !=
      0) {
    free(str);
    return -15;
  }
  free(str);
  str = str_format("%e|%8e|%08e|%.e|%8.e|%.3e|%8.3e|%08.3e|%*e|%0*e", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-1.012345e+01|-1.012345e+01|-1.012345e+01|-1e+01|  "
                  "-1e+01|-1.012e+01|-1.012e+01|-1.012e+01|-1.012345e+01|-1."
                  "012345e+01") != 0) {
    free(str);
    return -16;
  }
  free(str);
  str = str_format("%e|%8e|%08e|%.e|%8.e|%.3e|%8.3e|%08.3e|%*e|%0*e", 0.0, 0.0,
                   0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 16, 0.0, 16, 0.0);
  if (strcmp(str, "0.000000e+00|0.000000e+00|0.000000e+00|0e+00|   "
                  "0e+00|0.000e+00|0.000e+00|0.000e+00|    "
                  "0.000000e+00|00000.000000e+00") != 0) {
    free(str);
    return -17;
  }
  free(str);
  // Test %g
  str = str_format("%g|%8g|%08g|%.g|%8.g|%.3g|%8.3g|%08.3g|%*g|%0*g", 10.12345,
                   10.12345, 10.12345, 10.12345, 10.12345, 10.12345, 10.12345,
                   10.12345, 8, 10.12345, 8, 10.12345);
  if (strcmp(str, "10.1235| 10.1235|010.1235|1e+01|   1e+01|10.1|    "
                  "10.1|000010.1| 10.1235|010.1235") != 0) {
    free(str);
    return -18;
  }
  free(str);
  str = str_format("%g|%8g|%08g|%.g|%8.g|%.3g|%8.3g|%08.3g|%*g|%0*g", -10.12345,
                   -10.12345, -10.12345, -10.12345, -10.12345, -10.12345,
                   -10.12345, -10.12345, 8, -10.12345, 8, -10.12345);
  if (strcmp(str, "-10.1235|-10.1235|-10.1235|-1e+01|  -1e+01|-10.1|   "
                  "-10.1|-00010.1|-10.1235|-10.1235") != 0) {
    free(str);
    return -19;
  }
  free(str);
  str = str_format("%g|%8g|%08g|%.g|%8.g|%.3g|%8.3g|%08.3g|%*g|%0*g", 0.0, 0.0,
                   0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 16, 0.0, 16, 0.0);
  if (strcmp(
          str,
          "0|       0|00000000|0|       0|0|       0|00000000|               "
          "0|0000000000000000") != 0) {
    free(str);
    return -20;
  }
  free(str);
  // Test %g with trailing zeros
  str = str_format("%.14g", 1.0);
  if (strcmp(str, "1") != 0) {
    free(str);
    return -21;
  }
  free(str);
  // Test %g with big number
  str = str_format("%.14g", 10000000000.0);
  if (strcmp(str, "10000000000") != 0) {
    free(str);
    return -22;
  }
  free(str);
  // Test %g with a precision argument
  str = str_format("%.*g", 4, 10.234);
  if (strcmp(str, "10.23") != 0) {
    free(str);
    return -23;
  }
  free(str);
  // Test length modifiers
  str = str_format("%d %ld %lld %qd %u %lu %llu %qu", 10, 100, 4294967296,
                   4294967296, 10, 100, 4294967296, 4294967296);
  if (strcmp(str,
             "10 100 4294967296 4294967296 10 100 4294967296 4294967296") !=
      0) {
    free(str);
    return -24;
  }
  free(str);
  // Test %.50s with a long string
  str = str_format("%.50s",
                   "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWXYZ");
  if (strcmp(str, "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWX") != 0) {
    free(str);
    return -25;
  }
  free(str);
  // Test precision for %x
  str = str_format("%.8x-%.8x-%.2x", 10, 9999999, 9999999);
  if (strcmp(str, "0000000a-0098967f-98967f") != 0) {
    free(str);
    return -26;
  }
  free(str);
  // Test unknown specifier skip
  str = str_format("%I");
  if (strcmp(str, "I") != 0) {
    free(str);
    return -27;
  }
  free(str);
  // Test %s with padding
  const char *s = "Hello";
  str = str_format("[%10s]", s);
  if (strcmp(str, "[     Hello]") != 0) {
    free(str);
    return -28;
  }
  free(str);
  str = str_format("[%-10s]", s);
  if (strcmp(str, "[Hello     ]") != 0) {
    free(str);
    return -29;
  }
  free(str);
  str = str_format("[%*s]", 10, s);
  if (strcmp(str, "[     Hello]") != 0) {
    free(str);
    return -30;
  }
  free(str);
  str = str_format("[%-*s]", 10, s);
  if (strcmp(str, "[Hello     ]") != 0) {
    free(str);
    return -31;
  }
  free(str);
  // Test %p with padding
  str = str_format("%90p", &str);
  if (strlen(str) != 90) {
    free(str);
    return -32;
  }
  free(str);
  // Test sign prepend
  str = str_format("%+08d", 31501);
  if (strcmp(str, "+0031501") != 0) {
    free(str);
    return -33;
  }
  free(str);
  str = str_format("%+08d", -31501);
  if (strcmp(str, "-0031501") != 0) {
    free(str);
    return -34;
  }
  free(str);
  // P2G's armor shop displays signed resistances with %+3d.
  str = str_format("[%+3d][%+3d][%+3d][%+3d][%+d]", 3, 0, -2, 123, 0);
  if (strcmp(str, "[ +3][ +0][ -2][+123][+0]") != 0) {
    free(str);
    return -1001;
  }
  free(str);
  // Zero padding follows the sign, including zero and negative values.
  str = str_format("[%+08d][%05d]", 0, -12);
  if (strcmp(str, "[+0000000][-0012]") != 0) {
    free(str);
    return -1002;
  }
  free(str);
  // Explicit precision overrides zero padding; the sign counts in width.
  str = str_format("[%+6.3d][%+06.3d][%+3.0d]", -5, 5, 0);
  if (strcmp(str, "[  -005][  +005][  +]") != 0) {
    free(str);
    return -1003;
  }
  free(str);
  // Test h length modifier (%hd, %hu, %hx, %ho)
  str = str_format("%hd", 42);
  if (strcmp(str, "42") != 0) {
    free(str);
    return -35;
  }
  free(str);
  str = str_format("%hd", -42);
  if (strcmp(str, "-42") != 0) {
    free(str);
    return -36;
  }
  free(str);
  // Truncation: 32768 wraps to -32768 as signed short
  str = str_format("%hd", 32768);
  if (strcmp(str, "-32768") != 0) {
    free(str);
    return -37;
  }
  free(str);
  // Truncation: 65535 is -1 as signed short
  str = str_format("%hd", 65535);
  if (strcmp(str, "-1") != 0) {
    free(str);
    return -38;
  }
  free(str);
  str = str_format("%hu", 65535);
  if (strcmp(str, "65535") != 0) {
    free(str);
    return -39;
  }
  free(str);
  // Truncation: 65536 wraps to 0 as unsigned short
  str = str_format("%hu", 65536);
  if (strcmp(str, "0") != 0) {
    free(str);
    return -40;
  }
  free(str);
  str = str_format("%hx", 0x1234);
  if (strcmp(str, "1234") != 0) {
    free(str);
    return -41;
  }
  free(str);
  // Truncation: upper bits dropped
  str = str_format("%hx", 0x12345);
  if (strcmp(str, "2345") != 0) {
    free(str);
    return -42;
  }
  free(str);
  // Test hh length modifier (%hhd, %hhu, %hhx, %hho)
  str = str_format("%hhd", 127);
  if (strcmp(str, "127") != 0) {
    free(str);
    return -43;
  }
  free(str);
  str = str_format("%hhd", -128);
  if (strcmp(str, "-128") != 0) {
    free(str);
    return -44;
  }
  free(str);
  // Truncation: 128 wraps to -128 as signed char
  str = str_format("%hhd", 128);
  if (strcmp(str, "-128") != 0) {
    free(str);
    return -45;
  }
  free(str);
  // Truncation: 255 is -1 as signed char
  str = str_format("%hhd", 255);
  if (strcmp(str, "-1") != 0) {
    free(str);
    return -46;
  }
  free(str);
  // Truncation: 256 wraps to 0
  str = str_format("%hhd", 256);
  if (strcmp(str, "0") != 0) {
    free(str);
    return -47;
  }
  free(str);
  str = str_format("%hhu", 255);
  if (strcmp(str, "255") != 0) {
    free(str);
    return -48;
  }
  free(str);
  // Truncation: 256 wraps to 0 as unsigned char
  str = str_format("%hhu", 256);
  if (strcmp(str, "0") != 0) {
    free(str);
    return -49;
  }
  free(str);
  // -1 as unsigned char is 255
  str = str_format("%hhu", -1);
  if (strcmp(str, "255") != 0) {
    free(str);
    return -50;
  }
  free(str);
  str = str_format("%hhx", 0xAB);
  if (strcmp(str, "ab") != 0) {
    free(str);
    return -51;
  }
  free(str);
  // Truncation: upper bits dropped
  str = str_format("%hhx", 0x1AB);
  if (strcmp(str, "ab") != 0) {
    free(str);
    return -52;
  }
  free(str);
  // Test %ls (wide string, C locale)
  str = str_format("%ls", L"hello");
  if (strcmp(str, "hello") != 0) {
    free(str);
    return -53;
  }
  free(str);
  // Test %ls with ASCII-only wide string
  str = str_format("%ls", L"foo bar");
  if (strcmp(str, "foo bar") != 0) {
    free(str);
    return -54;
  }
  free(str);
  // Test %ls with empty wide string
  str = str_format("%ls", L"");
  if (strcmp(str, "") != 0) {
    free(str);
    return -55;
  }
  free(str);
  // Test %ls NULL
  str = str_format("%ls", (wchar_t *)NULL);
  if (strcmp(str, "(null)") != 0) {
    free(str);
    return -56;
  }
  free(str);
  // Test %ls embedded in a larger format string
  str = str_format("pre-%ls-post", L"mid");
  if (strcmp(str, "pre-mid-post") != 0) {
    free(str);
    return -57;
  }
  free(str);

  return 0;
}

int test_sscanf() {
  int a, b;
  short c, d;
  float f;
  double lf;
  char str[256], str1[16];
  int matched = sscanf("1.23", "%d.%d", &a, &b);
  if (!(matched == 2 && a == 1 && b == 23))
    return -1;
  matched = sscanf("abc111.42", "abc%d.%d", &a, &b);
  if (!(matched == 2 && a == 111 && b == 42))
    return -2;
  matched = sscanf("abc", "%d.%d", &a, &b);
  if (matched != 0)
    return -3;
  matched = sscanf("abc,8", "%[^,],%d", str, &b);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && b == 8))
    return -4;
  matched = sscanf("9,10", "%hi,%i", &c, &a);
  if (!(matched == 2 && c == 9 && a == 10))
    return -5;
  matched = sscanf("DUMMY", "%d", &a);
  if (matched != 0)
    return -6;
  matched = sscanf("+10 -10", "%d %d", &a, &b);
  if (!(matched == 2 && a == 10 && b == -10))
    return -7;
  matched = sscanf("+10 -10", "%hd %hd", &c, &d);
  if (!(matched == 2 && c == 10 && d == -10))
    return -9;
  matched = sscanf("3000\\t4", "%d %d", &a, &b);
  if (!(matched == 1 && a == 3000))
    return -10;
  matched = sscanf("0xFF0000", "%08x", &a);
  if (!(matched == 1 && a == 16711680))
    return -11;
  matched = sscanf("ABC\t1\t", "%s %f", str, &f);
  if (!(matched == 2 && strcmp(str, "ABC") == 0 && f == 1.0))
    return -12;
  matched = sscanf("ABC   1\t", "%s\t%f", str, &f);
  if (!(matched == 2 && strcmp(str, "ABC") == 0 && f == 1.0))
    return -13;
  matched = sscanf("MAX\t\t\t48.0\r\n", "%s %f", str, &f);
  if (!(matched == 2 && strcmp(str, "MAX") == 0 && f == 48.0))
    return -14;
  matched = sscanf("011", "%i", &a);
  if (!(matched == 1 && a == 9))
    return -15;
  matched = sscanf("09", "%i", &a);
  if (!(matched == 1 && a == 0))
    return -16;
  matched = sscanf("FF00", "%2x%2x", &a, &b);
  if (!(matched == 2 && a == 255 && b == 0))
    return -17;
  matched = sscanf("aa", "%10x", &a);
  if (!(matched == 1 && a == 170))
    return -18;
  matched = sscanf("3.14159265359", "%lf", &lf);
  if (!(matched == 1 && lf == 3.14159265359))
    return -19;
  matched = sscanf("hello123", "%[a-z]", str);
  if (!(matched == 1 && strcmp(str, "hello") == 0))
    return -20;
  matched = sscanf("abc123", "%[^0-9]", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -21;
  matched = sscanf("-123", "%[-0-9]", str);
  if (!(matched == 1 && strcmp(str, "-123") == 0))
    return -22;
  matched = sscanf("a-b", "%[a-z-]", str);
  if (!(matched == 1 && strcmp(str, "a-b") == 0))
    return -23;
  matched = sscanf("123", "%[^0-9]", str);
  if (matched != 0)
    return -24;
  matched = sscanf("Var_123 =", "%[A-Za-z0-9_]", str);
  if (!(matched == 1 && strcmp(str, "Var_123") == 0))
    return -25;
  matched = sscanf("NAME", "%s", str);
  if (!(matched == 1 && strcmp(str, "NAME") == 0))
    return -26;
  matched = sscanf("   NAME", "%s", str);
  if (!(matched == 1 && strcmp(str, "NAME") == 0))
    return -27;
  matched = sscanf("A B", "%s %s", str, str1);
  if (!(matched == 2 && strcmp(str, "A") == 0 && strcmp(str1, "B") == 0))
    return -28;
  matched = sscanf("numJoints 110\n", " numJoints %d", &a);
  if (!(matched == 1 && a == 110))
    return -29;
  float f1, f2, f3, f4, f5, f6;
  matched = sscanf(
      "	\"origin\"	-1 ( 0 0 0 ) ( -0.7071067095 0 0 )		// ",
      "%s %d ( %f %f %f ) ( %f %f %f )", str, &a, &f1, &f2, &f3, &f4, &f5, &f6);
  if (!(matched == 8 && strcmp(str, "\"origin\"") == 0 && a == -1 && f1 == 0 &&
        fabs(f4 + 0.7071067095) < 1e-10 && f6 == 0))
    return -30;
  // '%g' test cases
  matched = sscanf("123", "%g", &f);
  if (!(matched == 1 && f == 123.0f))
    return -31;
  matched = sscanf("1.23", "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -32;
  matched = sscanf("1.23e-4", "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23e-4f) < 1e-8f))
    return -33;
  matched = sscanf("1.23E4", "%g", &f);
  if (!(matched == 1 && fabs(f - 12300.0f) < 1e-5f))
    return -34;
  matched = sscanf("+1.23", "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -35;
  matched = sscanf("-1.23", "%g", &f);
  if (!(matched == 1 && fabs(f - -1.23f) < 1e-5f))
    return -36;
  matched = sscanf(".5", "%g", &f);
  if (!(matched == 1 && fabs(f - 0.5f) < 1e-5f))
    return -37;
  matched = sscanf("-.5", "%g", &f);
  if (!(matched == 1 && fabs(f - -0.5f) < 1e-5f))
    return -38;
  matched = sscanf("1e5", "%g", &f);
  if (!(matched == 1 && fabs(f - 100000.0f) < 1e-5f))
    return -39;
  matched = sscanf("1.e5", "%g", &f);
  if (!(matched == 1 && fabs(f - 100000.0f) < 1e-5f))
    return -40;
  matched = sscanf("  1.23", "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -41;
  matched = sscanf("+1.23e+4", "%g", &f);
  if (!(matched == 1 && fabs(f - 12300.0f) < 1e-5f))
    return -42;
  matched = sscanf("-1.23e-4", "%g", &f);
  if (!(matched == 1 && fabs(f - -0.000123f) < 1e-8f))
    return -43;
  matched = sscanf("123.", "%g", &f);
  if (!(matched == 1 && f == 123.0f))
    return -44;
  // max_width for %[ specifier
  matched = sscanf("hello", "%3[a-z]", str);
  if (!(matched == 1 && strcmp(str, "hel") == 0))
    return -45;
  matched = sscanf("abcXYZ", "%3[a-z]%3[A-Z]", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "XYZ") == 0))
    return -46;
  matched = sscanf("abc,def", "%3[^,],%3[^,]", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -47;
  matched = sscanf("abcdef", "%3[a-z]", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -48;
  matched = sscanf("ab", "%5[a-z]", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -49;
  // width of 1
  matched = sscanf("abc", "%1[a-z]", str);
  if (!(matched == 1 && strcmp(str, "a") == 0))
    return -50;
  // negated set stopped by width, not by excluded char
  matched = sscanf("abcde", "%3[^X]", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -51;
  // negated set stopped by excluded char before width is reached
  matched = sscanf("abXde", "%5[^X]", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -52;
  // input length exactly equals width
  matched = sscanf("abc", "%3[a-z]", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -53;
  // width limits %[ leaving remainder for next conversion
  matched = sscanf("abcdef", "%3[a-z]%s", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -54;
  // first char not in set with width: no match
  matched = sscanf("123", "%3[a-z]", str);
  if (matched != 0)
    return -55;
  // %hu (unsigned short) edge cases
  unsigned short us, us2;
  matched = sscanf("0", "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -56;
  matched = sscanf("65535", "%hu", &us);
  if (!(matched == 1 && us == 65535))
    return -57;
  // Truncation: 65536 wraps to 0 as unsigned short
  matched = sscanf("65536", "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -58;
  // Truncation: 65537 wraps to 1 as unsigned short
  matched = sscanf("65537", "%hu", &us);
  if (!(matched == 1 && us == 1))
    return -59;
  matched = sscanf("100 200", "%hu %hu", &us, &us2);
  if (!(matched == 2 && us == 100 && us2 == 200))
    return -60;
  // width limits the conversion
  matched = sscanf("12345", "%3hu", &us);
  if (!(matched == 1 && us == 123))
    return -61;
  // %hhu (unsigned char) edge cases
  unsigned char uc, uc2;
  matched = sscanf("0", "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -62;
  matched = sscanf("255", "%hhu", &uc);
  if (!(matched == 1 && uc == 255))
    return -63;
  // Truncation: 256 wraps to 0 as unsigned char
  matched = sscanf("256", "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -64;
  // Truncation: 257 wraps to 1 as unsigned char
  matched = sscanf("257", "%hhu", &uc);
  if (!(matched == 1 && uc == 1))
    return -65;
  matched = sscanf("10 20", "%hhu %hhu", &uc, &uc2);
  if (!(matched == 2 && uc == 10 && uc2 == 20))
    return -66;
  // width limits the conversion
  matched = sscanf("12345", "%2hhu", &uc);
  if (!(matched == 1 && uc == 12))
    return -67;
  // Overflow above UINT_MAX: per C semantics, the input is parsed as
  // a wide unsigned and only the low bits are stored, so 0x100000000
  // gives 0 in both u16 and u8.
  matched = sscanf("4294967296", "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -68;
  matched = sscanf("4294967296", "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -69;
  // %hx (unsigned short, hex) truncation
  matched = sscanf("ffff", "%hx", &us);
  if (!(matched == 1 && us == 0xFFFF))
    return -70;
  // Truncation: 0x10000 wraps to 0 as unsigned short
  matched = sscanf("10000", "%hx", &us);
  if (!(matched == 1 && us == 0))
    return -71;
  // Truncation: 0x10001 wraps to 1 as unsigned short
  matched = sscanf("10001", "%hx", &us);
  if (!(matched == 1 && us == 1))
    return -72;
  // %hhx (unsigned char, hex) truncation
  matched = sscanf("ff", "%hhx", &uc);
  if (!(matched == 1 && uc == 0xFF))
    return -73;
  // Truncation: 0x100 wraps to 0 as unsigned char
  matched = sscanf("100", "%hhx", &uc);
  if (!(matched == 1 && uc == 0))
    return -74;
  // Truncation: 0x101 wraps to 1 as unsigned char
  matched = sscanf("101", "%hhx", &uc);
  if (!(matched == 1 && uc == 1))
    return -75;
  // %hd (signed short) edge cases
  short ss, ss2;
  matched = sscanf("0", "%hd", &ss);
  if (!(matched == 1 && ss == 0))
    return -76;
  matched = sscanf("32767", "%hd", &ss);
  if (!(matched == 1 && ss == 32767))
    return -77;
  matched = sscanf("-32768", "%hd", &ss);
  if (!(matched == 1 && ss == -32768))
    return -78;
  // Truncation: 32768 wraps to -32768 as signed short
  matched = sscanf("32768", "%hd", &ss);
  if (!(matched == 1 && ss == -32768))
    return -79;
  // Truncation: -32769 wraps to 32767 as signed short
  matched = sscanf("-32769", "%hd", &ss);
  if (!(matched == 1 && ss == 32767))
    return -80;
  matched = sscanf("-100 200", "%hd %hd", &ss, &ss2);
  if (!(matched == 2 && ss == -100 && ss2 == 200))
    return -81;
  // width limits the conversion
  matched = sscanf("12345", "%3hd", &ss);
  if (!(matched == 1 && ss == 123))
    return -82;
  // width counts the sign character
  matched = sscanf("-12345", "%4hd", &ss);
  if (!(matched == 1 && ss == -123))
    return -83;
  // %hhd (signed char) edge cases
  signed char sc, sc2;
  matched = sscanf("0", "%hhd", &sc);
  if (!(matched == 1 && sc == 0))
    return -84;
  matched = sscanf("127", "%hhd", &sc);
  if (!(matched == 1 && sc == 127))
    return -85;
  matched = sscanf("-128", "%hhd", &sc);
  if (!(matched == 1 && sc == -128))
    return -86;
  // Truncation: 128 wraps to -128 as signed char
  matched = sscanf("128", "%hhd", &sc);
  if (!(matched == 1 && sc == -128))
    return -87;
  // Truncation: -129 wraps to 127 as signed char
  matched = sscanf("-129", "%hhd", &sc);
  if (!(matched == 1 && sc == 127))
    return -88;
  matched = sscanf("-10 20", "%hhd %hhd", &sc, &sc2);
  if (!(matched == 2 && sc == -10 && sc2 == 20))
    return -89;
  // width limits the conversion
  matched = sscanf("12345", "%2hhd", &sc);
  if (!(matched == 1 && sc == 12))
    return -90;
  // width counts the sign character
  matched = sscanf("-12345", "%3hhd", &sc);
  if (!(matched == 1 && sc == -12))
    return -91;
  // max_width for %s specifier
  // width truncates a longer token
  matched = sscanf("abcdef", "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -92;
  // width larger than the token reads the whole token
  matched = sscanf("ab", "%5s", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -93;
  // width of 1
  matched = sscanf("abc", "%1s", str);
  if (!(matched == 1 && strcmp(str, "a") == 0))
    return -94;
  // input length exactly equals width
  matched = sscanf("abc", "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -95;
  // %s stopped by whitespace before width is reached
  matched = sscanf("ab cd", "%5s", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -96;
  // leading whitespace is skipped and does not count towards width
  matched = sscanf("   abcdef", "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -97;
  // width limits %s leaving remainder for next conversion
  matched = sscanf("abcdef", "%3s%s", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -98;
  // two width-limited %s split a single token
  matched = sscanf("abcdefgh", "%3s%3s", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -99;
  // width-truncated %s leaves the remainder for a following %d
  matched = sscanf("abc123", "%3s%d", str, &a);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && a == 123))
    return -100;
  return 0;
}

int test_swscanf() {
  int a, b;
  int matched = swscanf(L"1.23", L"%d.%d", &a, &b);
  if (!(matched == 2 && a == 1 && b == 23))
    return -1;
  matched = swscanf(L"str_01", L"str_%2d", &a);
  if (!(matched == 1 && a == 1))
    return -2;
  return 0;
}

int test_realloc() {
  void *ptr = realloc(NULL, 32);
  memmove(ptr, "abcd", 4);
  ptr = realloc(ptr, 64);
  int res = memcmp(ptr, "abcd", 4);
  free(ptr);
  return res == 0 ? 0 : -1;
}

int test_valloc() {
  void *ptr = valloc(1);
  // Assume at least 4Kb page size alignment
  if (((uintptr_t)ptr & 0xFFF) != 0) {
    return -1;
  }
  if (ptr == NULL)
    return -2;
  ptr = realloc(ptr, 16);
  if (ptr == NULL)
    return -3;
  free(ptr);
  return 0;
}

int test_allocation_failure_contracts() {
  void *(*volatile allocate)(size_t) = malloc;
  void *(*volatile zeroed)(size_t, size_t) = calloc;
  void *(*volatile aligned)(size_t) = valloc;
  void *(*volatile resize)(void *, size_t) = realloc;
  void *(*volatile resize_free)(void *, size_t) = reallocf;
  const size_t impossible = (size_t)-1;
  errno = 0;
  if (allocate(impossible) || errno != ENOMEM)
    return 1;
  errno = 0;
  if (zeroed(0x80000001u, 2) || errno != ENOMEM)
    return 2;
  errno = 0;
  if (aligned(impossible) || errno != ENOMEM)
    return 3;
  unsigned char *old = allocate(32);
  if (!old)
    return 4;
  memset(old, 0x5a, 32);
  errno = 0;
  if (resize(old, impossible) || errno != ENOMEM || malloc_size(old) < 32)
    return 5;
  for (int i = 0; i < 32; i++)
    if (old[i] != 0x5a)
      return 6;
  free(old);
  errno = 0;
  old = allocate(32);
  if (!old || resize_free(old, impossible) || errno != ENOMEM)
    return 7;
  // reallocf consumed old. Do not free or read it after failure.
  errno = EDOM;
  void *nil_free = NULL;
  free(nil_free);
  if (errno != EDOM)
    return 8;
  return 0;
}

int test_allocation_zero_and_resize_contracts() {
  void *(*volatile allocate)(size_t) = malloc;
  void *(*volatile zeroed)(size_t, size_t) = calloc;
  void *(*volatile resize)(void *, size_t) = realloc;
  errno = EDOM;
  void *a = allocate(0), *b = zeroed(0, (size_t)-1);
  if (!a || !b || a == b || errno != EDOM)
    return 1;
  void *c = resize(a, 0);
  if (!c || c == a || errno != EDOM)
    return 2;
  free(b);
  free(c);
  if (errno != EDOM)
    return 3;
  unsigned char *bytes = zeroed(17, 3);
  if (!bytes || errno != EDOM)
    return 4;
  for (int i = 0; i < 51; i++)
    if (bytes[i])
      return 5;
  memcpy(bytes, "preserve", 8);
  unsigned char *grown = resize(bytes, 1024);
  if (!grown || memcmp(grown, "preserve", 8) || errno != EDOM)
    return 6;
  free(grown);
  return errno != EDOM ? 7 : 0;
}

static void *uf_allocation_errno_worker(void *unused) {
  (void)unused;
  // errno is initially available before the worker's first libc allocation.
  if (errno != 0)
    return (void *)1;
  void *(*volatile allocate)(size_t) = malloc;
  if (allocate((size_t)-1) || errno != ENOMEM)
    return (void *)2;
  void *p = allocate(32);
  if (!p || errno != ENOMEM)
    return (void *)3;
  free(p);
  return NULL;
}

int test_allocation_errno_is_thread_local() {
  pthread_t thread;
  void *result = (void *)99;
  errno = EDOM;
  if (pthread_create(&thread, NULL, uf_allocation_errno_worker, NULL) ||
      pthread_join(thread, &result))
    return 1;
  return result || errno != EDOM ? 2 : 0;
}

int TestApp_allocation_exhaustion_main(void) {
  // Keep these allocations alive until this dedicated test process exits.
  // Guest malloc reserves virtual ranges without touching their contents.
  void *(*volatile allocate)(size_t) = malloc;
  void *(*volatile resize)(void *, size_t) = realloc;
  void *(*volatile resize_free)(void *, size_t) = reallocf;
  const size_t block = 32u * 1024 * 1024;
  malloc_zone_t *zone = malloc_create_zone(0, 0);
  if (!zone)
    return 8;
  unsigned char *old = allocate(block);
  if (!old)
    return 1;
  memset(old, 0x5a, 64);
  int count = 0;
  while (allocate(block))
    if (++count > 128)
      return 2;
  if (count == 0 || errno != ENOMEM)
    return 3;
  char error_text[32];
  errno = EDOM;
  if (strerror_r(EINVAL, error_text, sizeof(error_text)) ||
      strcmp(error_text, "Invalid argument") || errno != EDOM)
    return 10;
  if (zone->malloc(zone, block * 2) || errno != ENOMEM ||
      malloc_create_zone(block * 2, 0) || errno != ENOMEM)
    return 9;
  malloc_destroy_zone(zone);
  if (resize(old, block * 2) || errno != ENOMEM || malloc_size(old) < block)
    return 4;
  for (int i = 0; i < 64; i++)
    if (old[i] != 0x5a)
      return 5;
  if (resize_free(old, block * 2) || errno != ENOMEM)
    return 6;
  // No block fitted before reallocf: this only succeeds if it freed old.
  if (!allocate(block))
    return 7;
  puts("PASS allocation exhaustion and reallocf release");
  return 0;
}

int test_atof() {
  if (atof("1") != 1)
    return -1;
  if (atof("-1") != -1)
    return -2;
  if (atof("01") != 1)
    return -3;
  if (atof("-01") != -1)
    return -4;
  if (atof("10") != 10)
    return -5;
  if (atof("-10") != -10)
    return -6;
  if (atof("010") != 10)
    return -7;
  if (atof("-010") != -10)
    return -8;
  if (atof("1.0") != 1)
    return -9;
  if (atof("-1.0") != -1)
    return -10;
  if (atof("01.0") != 1)
    return -11;
  if (atof("-01.0") != -1)
    return -12;
  if (atof("10.0") != 10)
    return -13;
  if (atof("-10.0") != -10)
    return -14;
  if (atof("010.0") != 10)
    return -15;
  if (atof("-010.0") != -10)
    return -16;
  if (atof("1.5") != 1.5)
    return -17;
  if (atof("-1.5") != -1.5)
    return -18;
  if (atof("01.5") != 1.5)
    return -19;
  if (atof("-01.5") != -1.5)
    return -20;
  if (atof("10.5") != 10.5)
    return -21;
  if (atof("-10.5") != -10.5)
    return -22;
  if (atof("010.5") != 10.5)
    return -23;
  if (atof("-010.5") != -10.5)
    return -24;
  if (atof("  +123.456e7with text right after") != 1234560000)
    return -25;
  if (atof("Text before a number 123.456") != 0)
    return -26;
  return 0;
}

int test_strtof() {
  char *text = "1";
  char *endptr;
  if (strtof(text, &endptr) != 1.0 || endptr != text + 1)
    return -1;
  text = "-1";
  if (strtof(text, &endptr) != -1.0 || endptr != text + 2)
    return -2;
  text = "01";
  if (strtof(text, &endptr) != 1.0 || endptr != text + 2)
    return -3;
  text = "-01";
  if (strtof(text, &endptr) != -1.0 || endptr != text + 3)
    return -4;
  text = "10";
  if (strtof(text, &endptr) != 10.0 || endptr != text + 2)
    return -5;
  text = "-10";
  if (strtof(text, &endptr) != -10.0 || endptr != text + 3)
    return -6;
  text = "010";
  if (strtof(text, &endptr) != 10.0 || endptr != text + 3)
    return -7;
  text = "-010";
  if (strtof(text, &endptr) != -10.0 || endptr != text + 4)
    return -8;
  text = "1.0";
  if (strtof(text, &endptr) != 1.0 || endptr != text + 3)
    return -9;
  text = "-1.0";
  if (strtof(text, &endptr) != -1.0 || endptr != text + 4)
    return -10;
  text = "01.0";
  if (strtof(text, &endptr) != 1.0 || endptr != text + 4)
    return -11;
  text = "-01.0";
  if (strtof(text, &endptr) != -1.0 || endptr != text + 5)
    return -12;
  text = "10.0";
  if (strtof(text, &endptr) != 10.0 || endptr != text + 4)
    return -13;
  text = "-10.0";
  if (strtof(text, &endptr) != -10.0 || endptr != text + 5)
    return -14;
  text = "010.0";
  if (strtof(text, &endptr) != 10.0 || endptr != text + 5)
    return -15;
  text = "-010.0";
  if (strtof(text, &endptr) != -10.0 || endptr != text + 6)
    return -16;
  text = "1.5";
  if (strtof(text, &endptr) != 1.5 || endptr != text + 3)
    return -17;
  text = "-1.5";
  if (strtof(text, &endptr) != -1.5 || endptr != text + 4)
    return -18;
  text = "01.5";
  if (strtof(text, &endptr) != 1.5 || endptr != text + 4)
    return -19;
  text = "-01.5";
  if (strtof(text, &endptr) != -1.5 || endptr != text + 5)
    return -20;
  text = "10.5";
  if (strtof(text, &endptr) != 10.5 || endptr != text + 4)
    return -21;
  text = "-10.5";
  if (strtof(text, &endptr) != -10.5 || endptr != text + 5)
    return -22;
  text = "010.5";
  if (strtof(text, &endptr) != 10.5 || endptr != text + 5)
    return -23;
  text = "-010.5";
  if (strtof(text, &endptr) != -10.5 || endptr != text + 6)
    return -24;
  text = "  +123.456e7with text right after";
  if (strtof(text, &endptr) != 1234560000.0 || endptr != text + 12)
    return -25;
  text = "Text before a number 123.456";
  if (strtof(text, &endptr) != 0.0 || endptr != text + 0)
    return -26;
  text = "1.5";
  if (strtof(text, NULL) != 1.5)
    return -27;
  return 0;
}

int test_strtoul() {
  char *text = "0xcccccccc";
  char *endptr;
  if (strtoul(text, &endptr, 16) != 3435973836 || endptr != text + 10) {
    return -1;
  }
  text = "12345";
  if (strtoul(text, &endptr, 10) != 12345UL || endptr != text + 5) {
    return -2;
  }
  text = "123abc";
  if (strtoul(text, &endptr, 10) != 123UL || endptr != text + 3) {
    return -3;
  }
  text = "abc";
  if (strtoul(text, &endptr, 10) != 0UL || endptr != text) {
    return -4;
  }
  text = "-1";
  if (strtoul(text, &endptr, 10) != (unsigned long)-1 || endptr != text + 2) {
    return -5;
  }
  text = "Ff";
  if (strtoul(text, &endptr, 16) != 255UL || endptr != text + 2) {
    return -6;
  }
  text = "   +42abc";
  if (strtoul(text, &endptr, 10) != 42UL || endptr != text + 6) {
    return -7;
  }
#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
  // Test for overflow. "4294967296" is ULONG_MAX + 1 on a 32-bit system.
  text = "4294967296";
  if (strtoul(text, &endptr, 10) != 4294967295 || endptr != text + 10) {
    return -8;
  }
#endif
  text = "4294967295";
  if (strtoul(text, &endptr, 10) != 4294967295 || endptr != text + 10) {
    return -9;
  }
  text = "15";
  if (strtoul(text, &endptr, 0) != 15UL || endptr != text + 2) {
    return -10;
  }
  text = "017"; // octal: 1*8 + 7 = 15
  if (strtoul(text, &endptr, 0) != 15UL || endptr != text + 3) {
    return -11;
  }
  text = "0x0F";
  if (strtoul(text, &endptr, 0) != 15UL || endptr != text + 4) {
    return -12;
  }
  text = "";
  if (strtoul(text, &endptr, 10) != 0UL || endptr != text) {
    return -13;
  }
  text = "   ";
  if (strtoul(text, &endptr, 10) != 0UL || endptr != text) {
    return -14;
  }
  text = "1101"; // binary: 8 + 4 + 1 = 13
  if (strtoul(text, &endptr, 2) != 13UL || endptr != text + 4) {
    return -15;
  }
  text = "zZ"; // base 36: 35*36 + 35 = 1295
  if (strtoul(text, &endptr, 36) != 1295UL || endptr != text + 2) {
    return -16;
  }
  text = "77"; // octal: 7*8 + 7 = 63
  if (strtoul(text, &endptr, 8) != 63UL || endptr != text + 2) {
    return -17;
  }
  return 0;
}

#ifdef DEFINE_ME_WHEN_BUILDING_ON_MACOS
#define MAX_LONG 9223372036854775807
#else
#define MAX_LONG 2147483647
#endif

int test_strtol() {
  const char *p = "10 200000000000000000000000000000  30   -40    junk";
  long res[] = {10, MAX_LONG, 30, -40, 0};
  int count = sizeof(res) / sizeof(long);
  for (int i = 0; i < count; i++) {
    char *endp = NULL;
    long l = strtol(p, &endp, 10);
    if (p == endp)
      break;
    p = endp;
    if (res[i] != l) {
      return -(i + 1);
    }
  }
  p = "-";
  long l = strtol(p, NULL, 0);
  if (l != 0) {
    return -count;
  }
  p = "+";
  l = strtol(p, NULL, 0);
  if (l != 0) {
    return -(count + 1);
  }
  p = "+-+";
  l = strtol(p, NULL, 0);
  if (l != 0) {
    return -(count + 2);
  }
  p = "0x123 +0x123 -0x123";
  long res2[] = {291, 291, -291};
  int count2 = sizeof(res2) / sizeof(long);
  for (int i = 0; i < count2; i++) {
    char *endp = NULL;
    l = strtol(p, &endp, 16);
    if (p == endp)
      break;
    p = endp;
    if (res2[i] != l) {
      return -(count + 2 + i + 1);
    }
  }
  return 0;
}

// This is a simplified version of test_strtol()
int test_wcstol() {
  const wchar_t *subjects[] = {L"10", L"200000000000000000000000000000",
                               L"  30", L"   -40"};
  long res[] = {10, MAX_LONG, 30, -40};
  int count = sizeof(res) / sizeof(long);
  for (int i = 0; i < count; i++) {
    long l = wcstol(subjects[i], NULL, 10);
    if (res[i] != l) {
      return -(i + 1);
    }
  }
  long l;
  const wchar_t *subjects2[] = {L"0x123", L"+0x123", L"-0x123"};
  long res2[] = {291, 291, -291};
  int count2 = sizeof(res2) / sizeof(long);
  for (int i = 0; i < count2; i++) {
    l = wcstol(subjects2[i], NULL, 16);
    if (res2[i] != l) {
      return -(count + 2 + i + 1);
    }
  }
  return 0;
}

int test_getcwd_chdir() {
  char buf[256];
  char *buf2 = getcwd(buf, sizeof buf);
  if (!buf2 || buf2 != buf || strcmp("/", buf))
    return -1;

  if (!chdir("does_not_exist") || !chdir("/does/not/exist"))
    return -1;

  if (chdir("/var/"))
    return -1;

  if (chdir("mobile/Applications"))
    return -1;

  char *buf3 = getcwd(NULL, 0);
  if (!buf3 || strcmp("/var/mobile/Applications", buf3))
    return -1;
  free(buf3);

  char *buf5 = getcwd(buf, 4); // too small
  if (buf5)
    return -1;

  if (chdir(".."))
    return -1;

  char *buf6 = getcwd(buf, sizeof buf);
  if (!buf6 || buf6 != buf || strcmp("/var/mobile", buf6))
    return -1;

  FILE *fake_file = fopen("TestApp", "r"); // doesn't exist in this directory
  if (fake_file) {
    fclose(fake_file);
    return -1;
  }

  if (chdir("Applications/00000000-0000-0000-0000-000000000000/TestApp.app"))
    return -1;

  if (!chdir("TestApp")) // isn't a directory
    return -1;

  FILE *real_file = fopen("TestApp", "r");
  if (!real_file)
    return -1;
  fclose(real_file);

  if (chdir("/"))
    return -1;

  return 0;
}

sem_t *semaphore;
int shared_int = 0;

void sem_thread_func() {
  while (1) {
    if (sem_trywait(semaphore) == -1) {
      return;
    }
    shared_int = -1;
    sem_post(semaphore);
    usleep(100);
  }
}

int test_sem() {
  semaphore = sem_open("sem_test", O_CREAT, 0644, 1);
  if (semaphore == SEM_FAILED) {
    printf("Error opening semaphore\n");
    return -1;
  }

  pthread_t *my_thread = (pthread_t *)malloc(sizeof(pthread_t));
  pthread_create(my_thread, NULL, (void *)sem_thread_func, NULL);
  usleep(200);

  sem_wait(semaphore);

  shared_int = 1;
  usleep(200);

  sem_close(semaphore);
  sem_unlink("sem_test");
  if (shared_int != 1) {
    return -1;
  }

  // Check that reopen is fine
  semaphore = sem_open("sem_test", O_CREAT, 0644, 1);
  if (semaphore == SEM_FAILED) {
    printf("Error opening semaphore\n");
    return -1;
  }

  // Sem @ 0
  if (sem_trywait(semaphore) == -1) {
    return -1;
  }

  // Sem still @ 0, should not lock
  if (sem_trywait(semaphore) == 0) {
    return -1;
  }

  // Sem @ 1, should be able to relock
  sem_post(semaphore);
  if (sem_trywait(semaphore) == -1) {
    return -1;
  }

  sem_close(semaphore);
  sem_unlink("sem_test");
  return 0;
}

sem_t *mt_semaphore;

void mtsem_thread() {
  sem_wait(mt_semaphore);
  sem_post(mt_semaphore);
}

int test_mtsem() {
  mt_semaphore = sem_open("mtsem_test", O_CREAT, 0644, 0);
  if (mt_semaphore == SEM_FAILED) {
    printf("Error opening semaphore\n");
    return -1;
  }

  pthread_t *my_thread = (pthread_t *)malloc(sizeof(pthread_t));
  pthread_create(my_thread, NULL, (void *)mtsem_thread, NULL);

  pthread_t *my_thread2 = (pthread_t *)malloc(sizeof(pthread_t));
  pthread_create(my_thread2, NULL, (void *)mtsem_thread, NULL);

  usleep(1);
  usleep(1);

  sem_post(mt_semaphore);
  pthread_join(*my_thread, NULL);
  pthread_join(*my_thread2, NULL);
  return 0;
}

sem_t *thread_suspend_semaphore;
sem_t *thread_suspend_ready;
int thread_suspend_flag = 0;

void *thread_suspend_thread_func(void *arg) {
  sem_post(thread_suspend_ready);
  sem_wait(thread_suspend_semaphore);
  thread_suspend_flag = 1;
  return NULL;
}

int test_thread_suspend_resume() {
  thread_suspend_flag = 0;

  thread_suspend_semaphore = sem_open("thread_suspend_test", O_CREAT, 0644, 0);
  if (thread_suspend_semaphore == SEM_FAILED) {
    printf("Error opening semaphore\n");
    return -1;
  }
  thread_suspend_ready = sem_open("thread_suspend_ready", O_CREAT, 0644, 0);
  if (thread_suspend_ready == SEM_FAILED) {
    sem_close(thread_suspend_semaphore);
    sem_unlink("thread_suspend_test");
    printf("Error opening ready semaphore\n");
    return -2;
  }

  pthread_t thr;
  if (pthread_create(&thr, NULL, thread_suspend_thread_func, NULL) != 0)
    return -3;

  // Wait until the worker has reached sem_wait().
  sem_wait(thread_suspend_ready);

  mach_port_t thr_port = pthread_mach_thread_np(thr);
  if (thread_suspend(thr_port) != KERN_SUCCESS)
    return -4;

  // Check that run_state reports TH_STATE_WAITING while suspended.
  thread_basic_info_data_t thr_info;
  mach_msg_type_number_t info_count = THREAD_BASIC_INFO_COUNT;
  if (thread_info(thr_port, THREAD_BASIC_INFO, (thread_info_t)&thr_info,
                  &info_count) != KERN_SUCCESS)
    return -5;
  if (thr_info.run_state != TH_STATE_WAITING)
    return -6;

  // Post the semaphore while the thread is suspended - it should not wake up.
  sem_post(thread_suspend_semaphore);
  sched_yield();

  if (thread_suspend_flag != 0)
    return -7;

  if (thread_resume(thr_port) != KERN_SUCCESS)
    return -8;

  pthread_join(thr, NULL);

  if (thread_suspend_flag != 1)
    return -9;

  sem_close(thread_suspend_semaphore);
  sem_unlink("thread_suspend_test");
  sem_close(thread_suspend_ready);
  sem_unlink("thread_suspend_ready");
  return 0;
}

int done = 0, done2 = 0;
pthread_mutex_t m;
pthread_cond_t c, c2;

void thr_exit() {
  pthread_mutex_lock(&m);
  done = 1;
  pthread_cond_signal(&c);
  pthread_mutex_unlock(&m);
}

void *child(void *arg) {
  thr_exit();
  return NULL;
}

void *child2(void *arg) {
  pthread_mutex_lock(&m);
  while (done2 == 0) {
    pthread_cond_wait(&c2, &m);
  }
  pthread_mutex_unlock(&m);
  return NULL;
}

void thr_join() {
  pthread_mutex_lock(&m);
  while (done == 0) {
    pthread_cond_wait(&c, &m);
  }
  pthread_mutex_unlock(&m);
}

int test_cond_var() {
  pthread_t p;

  pthread_mutex_init(&m, NULL);
  pthread_cond_init(&c, NULL);

  pthread_create(&p, NULL, child, NULL);
  thr_join();

  if (done != 1)
    return -1;

  // Should wake up all threads
  int result;
  pthread_t p1, p2, p3;
  result = pthread_cond_init(&c2, NULL);
  if (result != 0)
    return -2;
  pthread_create(&p1, NULL, child2, NULL);
  pthread_create(&p2, NULL, child2, NULL);
  pthread_create(&p3, NULL, child2, NULL);
  usleep(100);
  result = pthread_mutex_lock(&m);
  if (result != 0)
    return -3;
  done2 = 1;
  result = pthread_cond_broadcast(&c2);
  if (result != 0)
    return -4;
  result = pthread_mutex_unlock(&m);
  if (result != 0)
    return -5;
  pthread_join(p1, NULL);
  pthread_join(p2, NULL);
  pthread_join(p3, NULL);

  return 0;
}

pthread_mutex_t m_static = PTHREAD_MUTEX_INITIALIZER;
pthread_cond_t c_static = PTHREAD_COND_INITIALIZER,
               c2_static = PTHREAD_COND_INITIALIZER;

void thr_exit_static() {
  pthread_mutex_lock(&m_static);
  done = 1;
  pthread_cond_signal(&c_static);
  pthread_mutex_unlock(&m_static);
}

void *child_static(void *arg) {
  thr_exit_static();
  return NULL;
}

void *child2_static(void *arg) {
  pthread_mutex_lock(&m_static);
  while (done2 == 0) {
    pthread_cond_wait(&c2_static, &m_static);
  }
  pthread_mutex_unlock(&m_static);
  return NULL;
}

void thr_join_static() {
  pthread_mutex_lock(&m_static);
  while (done == 0) {
    pthread_cond_wait(&c_static, &m_static);
  }
  pthread_mutex_unlock(&m_static);
}

int test_cond_var_static() {
  pthread_t p;
  done = 0;
  done2 = 0;

  // We test that statically allocated cond vars and mutexes work
  // by using them without calling pthread_mutex_init and pthread_cond_init

  pthread_create(&p, NULL, child_static, NULL);
  thr_join_static();

  if (done != 1)
    return -1;

  // Should wake up all threads
  int result;
  pthread_t p1, p2, p3;
  pthread_create(&p1, NULL, child2_static, NULL);
  pthread_create(&p2, NULL, child2_static, NULL);
  pthread_create(&p3, NULL, child2_static, NULL);
  usleep(100);
  result = pthread_mutex_lock(&m_static);
  if (result != 0)
    return -2;
  done2 = 1;
  result = pthread_cond_broadcast(&c2_static);
  if (result != 0)
    return -3;
  result = pthread_mutex_unlock(&m_static);
  if (result != 0)
    return -4;
  pthread_join(p1, NULL);
  pthread_join(p2, NULL);
  pthread_join(p3, NULL);

  return 0;
}

// === pthread_cond_timedwait tests ===

struct timedwait_signaler_args {
  pthread_mutex_t *mu;
  pthread_cond_t *cv;
};

void *timedwait_signaler(void *arg) {
  struct timedwait_signaler_args *a = arg;
  usleep(20000); // 20ms - well before the 500ms deadline
  pthread_mutex_lock(a->mu);
  pthread_cond_signal(a->cv);
  pthread_mutex_unlock(a->mu);
  return NULL;
}

// Test : signal arrives before deadline - should return 0, not ETIMEDOUT
int test_cond_timedwait_signaled_before_timeout() {
  pthread_mutex_t mu;
  pthread_cond_t cv;
  if (pthread_mutex_init(&mu, NULL) != 0)
    return -1;
  if (pthread_cond_init(&cv, NULL) != 0)
    return -2;

  struct timedwait_signaler_args args = {&mu, &cv};
  pthread_t p;
  if (pthread_create(&p, NULL, timedwait_signaler, &args) != 0)
    return -3;

  struct timeval tv;
  gettimeofday(&tv, NULL);
  // Generous 500ms deadline
  struct timespec ts = {.tv_sec = tv.tv_sec,
                        .tv_nsec = tv.tv_usec * 1000 + 500000000};
  if (ts.tv_nsec >= 1000000000) {
    ts.tv_sec += 1;
    ts.tv_nsec -= 1000000000;
  }

  pthread_mutex_lock(&mu);
  int result = pthread_cond_timedwait(&cv, &mu, &ts);
  pthread_mutex_unlock(&mu);

  pthread_join(p, NULL);
  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);

  if (result != 0)
    return -4;
  return 0;
}

// Test : deadline already in the past - should return ETIMEDOUT immediately
int test_cond_timedwait_past_deadline() {
  pthread_mutex_t mu;
  pthread_cond_t cv;
  if (pthread_mutex_init(&mu, NULL) != 0)
    return -1;
  if (pthread_cond_init(&cv, NULL) != 0)
    return -2;

  // Use a timestamp far in the past
  struct timespec ts = {.tv_sec = 1, .tv_nsec = 0};

  pthread_mutex_lock(&mu);
  int result = pthread_cond_timedwait(&cv, &mu, &ts);
  pthread_mutex_unlock(&mu);

  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);

  if (result != ETIMEDOUT)
    return -3;
  return 0;
}

struct timedwait_broadcast_args {
  pthread_mutex_t *mu;
  pthread_cond_t *cv;
  int ready;
};

void *timedwait_broadcast_waiter(void *arg) {
  struct timedwait_broadcast_args *a = arg;
  struct timeval tv;
  gettimeofday(&tv, NULL);
  struct timespec ts = {.tv_sec = tv.tv_sec,
                        .tv_nsec = tv.tv_usec * 1000 + 500000000};
  if (ts.tv_nsec >= 1000000000) {
    ts.tv_sec += 1;
    ts.tv_nsec -= 1000000000;
  }
  pthread_mutex_lock(a->mu);
  while (a->ready == 0)
    pthread_cond_timedwait(a->cv, a->mu, &ts);
  pthread_mutex_unlock(a->mu);
  return NULL;
}

// Test: broadcast wakes all timedwait threads
int test_cond_timedwait_broadcast() {
  pthread_mutex_t mu;
  pthread_cond_t cv;
  if (pthread_mutex_init(&mu, NULL) != 0)
    return -1;
  if (pthread_cond_init(&cv, NULL) != 0)
    return -2;

  struct timedwait_broadcast_args args = {&mu, &cv, 0};
  pthread_t p1, p2, p3;
  if (pthread_create(&p1, NULL, timedwait_broadcast_waiter, &args) != 0)
    return -3;
  if (pthread_create(&p2, NULL, timedwait_broadcast_waiter, &args) != 0)
    return -4;
  if (pthread_create(&p3, NULL, timedwait_broadcast_waiter, &args) != 0)
    return -5;

  usleep(50000); // let all three threads reach their waits
  pthread_mutex_lock(&mu);
  args.ready = 1;
  int result = pthread_cond_broadcast(&cv);
  pthread_mutex_unlock(&mu);

  if (result != 0)
    return -6;

  pthread_join(p1, NULL);
  pthread_join(p2, NULL);
  pthread_join(p3, NULL);

  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);
  return 0;
}

// Test: timed-out state on a cond must not persist across calls. After one
// thread times out, a later signaled timedwait on the same cond must
// return 0, not ETIMEDOUT.
int test_cond_timedwait_flag_not_sticky() {
  pthread_mutex_t mu;
  pthread_cond_t cv;
  if (pthread_mutex_init(&mu, NULL) != 0)
    return -1;
  if (pthread_cond_init(&cv, NULL) != 0)
    return -2;

  // First call: force an immediate timeout.
  struct timespec past = {.tv_sec = 1, .tv_nsec = 0};
  pthread_mutex_lock(&mu);
  int first = pthread_cond_timedwait(&cv, &mu, &past);
  pthread_mutex_unlock(&mu);
  if (first != ETIMEDOUT)
    return -3;

  // Second call on the same cond: should be signaled and return 0.
  struct timedwait_signaler_args args = {&mu, &cv};
  pthread_t p;
  if (pthread_create(&p, NULL, timedwait_signaler, &args) != 0)
    return -4;

  struct timeval tv;
  gettimeofday(&tv, NULL);
  struct timespec ts = {.tv_sec = tv.tv_sec,
                        .tv_nsec = tv.tv_usec * 1000 + 500000000};
  if (ts.tv_nsec >= 1000000000) {
    ts.tv_sec += 1;
    ts.tv_nsec -= 1000000000;
  }

  pthread_mutex_lock(&mu);
  int second = pthread_cond_timedwait(&cv, &mu, &ts);
  pthread_mutex_unlock(&mu);

  pthread_join(p, NULL);
  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);

  if (second != 0)
    return -5;
  return 0;
}

struct timedwait_sibling_args {
  pthread_mutex_t *mu;
  pthread_cond_t *cv;
  long sec_offset;
  long ns_offset; // must be < 1000000000
  int result;
};

void *timedwait_sibling_sleeper(void *arg) {
  (void)arg;
  usleep(200000);
  return NULL;
}

void *timedwait_sibling_waiter(void *arg) {
  struct timedwait_sibling_args *a = arg;
  struct timeval tv;
  gettimeofday(&tv, NULL);
  struct timespec ts = {.tv_sec = tv.tv_sec + a->sec_offset,
                        .tv_nsec = tv.tv_usec * 1000 + a->ns_offset};
  if (ts.tv_nsec >= 1000000000) {
    ts.tv_sec += 1;
    ts.tv_nsec -= 1000000000;
  }
  pthread_mutex_lock(a->mu);
  a->result = pthread_cond_timedwait(a->cv, a->mu, &ts);
  pthread_mutex_unlock(a->mu);
  return NULL;
}

// Test: when one waiter times out, other waiters on the same cond must
// still be reachable by a later signal - i.e. a timeout on one thread
// must not drop sibling waiters from the queue.
int test_cond_timedwait_sibling_not_dropped() {
  pthread_mutex_t mu;
  pthread_cond_t cv;
  if (pthread_mutex_init(&mu, NULL) != 0)
    return -1;
  if (pthread_cond_init(&cv, NULL) != 0)
    return -2;

  struct timedwait_sibling_args long_args = {&mu, &cv, 3, 0, -999};
  struct timedwait_sibling_args short_args = {&mu, &cv, 0, 100000000, -999};
  pthread_t p_long, p_short, p_sleeper;
  if (pthread_create(&p_long, NULL, timedwait_sibling_waiter, &long_args) != 0)
    return -3;
  usleep(20000); // let the long-deadline waiter enter the wait first
  if (pthread_create(&p_short, NULL, timedwait_sibling_waiter, &short_args) !=
      0)
    return -4;
  // A thread unrelated to the cond var - keeps the scheduler ticking while
  // the waiters are blocked on Condition, and verifies that sibling fallout
  // from the timeout doesn't leak to unrelated threads.
  if (pthread_create(&p_sleeper, NULL, timedwait_sibling_sleeper, NULL) != 0)
    return -8;

  // Join the short waiter first. This forces the scheduler to actually
  // process short's timeout before we signal, so the bug path (which
  // clears the entire waiting queue on timeout) has a chance to fire.
  pthread_join(p_short, NULL);
  if (short_args.result != ETIMEDOUT)
    return -5;

  // Now signal. The long waiter must still be reachable.
  pthread_mutex_lock(&mu);
  int res = pthread_cond_signal(&cv);
  pthread_mutex_unlock(&mu);
  if (res != 0)
    return -6;

  pthread_join(p_long, NULL);
  pthread_join(p_sleeper, NULL);
  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);

  if (long_args.result != 0)
    return -7;
  return 0;
}

// === end pthread_cond_timedwait tests ===

pthread_mutex_t normal_mutex;
int normal_unlock_res = -1;

void *normal_unlocker(void *arg) {
  normal_unlock_res = pthread_mutex_unlock(&normal_mutex);
  return NULL;
}

int test_pthread_mutex_normal() {
  pthread_mutexattr_t attr;
  if (pthread_mutexattr_init(&attr) != 0)
    return -1;
  if (pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_NORMAL) != 0)
    return -2;
  if (pthread_mutex_init(&normal_mutex, &attr) != 0)
    return -3;
  if (pthread_mutexattr_destroy(&attr) != 0)
    return -4;

  if (pthread_mutex_lock(&normal_mutex) != 0)
    return -5;

  pthread_t p;
  if (pthread_create(&p, NULL, normal_unlocker, NULL) != 0)
    return -6;
  if (pthread_join(p, NULL) != 0)
    return -7;

  if (pthread_mutex_destroy(&normal_mutex) != 0)
    return -8;

  if (normal_unlock_res != 0)
    return -9;

  return 0;
}

pthread_mutex_t recursive_mutex;
int recursive_trylock_res = -1;

void *recursive_trylocker(void *arg) {
  recursive_trylock_res = pthread_mutex_trylock(&recursive_mutex);
  return NULL;
}

int test_pthread_mutex_recursive_trylock() {
  pthread_mutexattr_t attr;
  if (pthread_mutexattr_init(&attr) != 0)
    return -1;
  if (pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE) != 0)
    return -2;
  if (pthread_mutex_init(&recursive_mutex, &attr) != 0)
    return -3;
  if (pthread_mutexattr_destroy(&attr) != 0)
    return -4;

  if (pthread_mutex_trylock(&recursive_mutex) != 0)
    return -5;

  if (pthread_mutex_trylock(&recursive_mutex) != 0)
    return -6;

  pthread_t p;
  if (pthread_create(&p, NULL, recursive_trylocker, NULL) != 0)
    return -7;
  if (pthread_join(p, NULL) != 0)
    return -8;

  if (recursive_trylock_res != EBUSY)
    return -9;

  if (pthread_mutex_unlock(&recursive_mutex) != 0)
    return -10;
  if (pthread_mutex_unlock(&recursive_mutex) != 0)
    return -11;

  if (pthread_mutex_destroy(&recursive_mutex) != 0)
    return -12;

  return 0;
}

// === NSConditionLock tests ===

// Condition values used by the producer/consumer test below.
#define NSCL_NO_DATA 0
#define NSCL_HAS_DATA 1
#define NSCL_PC_ITERATIONS 5

// init defaults the condition to 0, while initWithCondition: sets it.
int test_NSConditionLock_init() {
  NSConditionLock *lock = [[NSConditionLock alloc] init];
  if (lock == nil)
    return -1;
  if ([lock condition] != 0)
    return -2;
  [lock release];

  NSConditionLock *lock_with_condition =
      [[NSConditionLock alloc] initWithCondition:42];
  if (lock_with_condition == nil)
    return -3;
  if ([lock_with_condition condition] != 42)
    return -4;
  [lock_with_condition release];
  return 0;
}

// Plain lock/unlock ignores the condition, while unlockWithCondition: sets it.
int test_NSConditionLock_lock_unlock() {
  NSConditionLock *lock = [[NSConditionLock alloc] initWithCondition:1];

  [lock lock];
  if ([lock condition] != 1)
    return -1;
  [lock unlock];
  if ([lock condition] != 1)
    return -2;

  [lock lock];
  [lock unlockWithCondition:7];
  if ([lock condition] != 7)
    return -3;

  [lock release];
  return 0;
}

// tryLockWhenCondition: only succeeds when the condition matches, and never
// blocks.
int test_NSConditionLock_tryLockWhenCondition() {
  NSConditionLock *lock = [[NSConditionLock alloc] initWithCondition:1];

  if ([lock tryLockWhenCondition:2])
    return -1;
  if (![lock tryLockWhenCondition:1])
    return -2;
  [lock unlockWithCondition:2];

  if (![lock tryLockWhenCondition:2])
    return -3;
  [lock unlockWithCondition:2];

  [lock release];
  return 0;
}

NSConditionLock *nscl_trylock_lock;
sem_t *nscl_trylock_acquired;
sem_t *nscl_trylock_release;

void *nsconditionlock_holder(void *arg) {
  (void)arg;
  [nscl_trylock_lock lock];
  sem_post(nscl_trylock_acquired);
  sem_wait(nscl_trylock_release);
  [nscl_trylock_lock unlock];
  return NULL;
}

// tryLock succeeds when the lock is free and fails when another thread holds
// it, without blocking.
int test_NSConditionLock_tryLock_contended() {
  nscl_trylock_lock = [[NSConditionLock alloc] init];

  if (![nscl_trylock_lock tryLock])
    return -1;
  [nscl_trylock_lock unlock];

  nscl_trylock_acquired = sem_open("nscl_trylock_acquired", O_CREAT, 0644, 0);
  if (nscl_trylock_acquired == SEM_FAILED)
    return -2;
  nscl_trylock_release = sem_open("nscl_trylock_release", O_CREAT, 0644, 0);
  if (nscl_trylock_release == SEM_FAILED)
    return -3;

  pthread_t p;
  if (pthread_create(&p, NULL, nsconditionlock_holder, NULL) != 0)
    return -4;

  // Wait until the other thread holds the lock.
  sem_wait(nscl_trylock_acquired);

  // tryLock must fail because the lock is held by another thread.
  if ([nscl_trylock_lock tryLock])
    return -5;

  // Let the holder release the lock and finish.
  sem_post(nscl_trylock_release);
  if (pthread_join(p, NULL) != 0)
    return -6;

  if (![nscl_trylock_lock tryLock])
    return -7;
  [nscl_trylock_lock unlock];

  sem_close(nscl_trylock_acquired);
  sem_unlink("nscl_trylock_acquired");
  sem_close(nscl_trylock_release);
  sem_unlink("nscl_trylock_release");
  [nscl_trylock_lock release];
  return 0;
}

NSConditionLock *nscl_pc_lock;
int nscl_pc_buffer;
int nscl_pc_consumed_sum;

void *nsconditionlock_producer(void *arg) {
  (void)arg;
  for (int i = 0; i < NSCL_PC_ITERATIONS; i++) {
    [nscl_pc_lock lockWhenCondition:NSCL_NO_DATA];
    nscl_pc_buffer = i;
    [nscl_pc_lock unlockWithCondition:NSCL_HAS_DATA];
  }
  return NULL;
}

// lockWhenCondition:/unlockWithCondition: hand off a single-slot buffer between
// a producer thread and the consuming main thread, blocking until the awaited
// condition is set.
int test_NSConditionLock_producer_consumer() {
  nscl_pc_lock = [[NSConditionLock alloc] initWithCondition:NSCL_NO_DATA];
  nscl_pc_consumed_sum = 0;

  pthread_t producer;
  if (pthread_create(&producer, NULL, nsconditionlock_producer, NULL) != 0)
    return -1;

  for (int i = 0; i < NSCL_PC_ITERATIONS; i++) {
    // Blocks until the producer makes data available.
    [nscl_pc_lock lockWhenCondition:NSCL_HAS_DATA];
    nscl_pc_consumed_sum += nscl_pc_buffer;
    [nscl_pc_lock unlockWithCondition:NSCL_NO_DATA];
  }

  if (pthread_join(producer, NULL) != 0)
    return -2;

  int expected = 0;
  for (int i = 0; i < NSCL_PC_ITERATIONS; i++)
    expected += i;
  if (nscl_pc_consumed_sum != expected)
    return -3;

  [nscl_pc_lock release];
  return 0;
}

int second_thread_thread_size_res = -1;

void *second_thread(void *arg) {
  size_t stack_size = pthread_get_stacksize_np(pthread_self());
  if (stack_size == 512 * 1024) {
    second_thread_thread_size_res = 0;
  }
  return NULL;
}

int test_pthread_get_stacksize_np() {
  size_t stack_size = pthread_get_stacksize_np(pthread_self());
  if (stack_size != 1024 * 1024 - getpagesize()) {
    return -1;
  }

  pthread_t p;
  if (pthread_create(&p, NULL, second_thread, NULL) != 0)
    return -2;
  if (pthread_join(p, NULL) != 0)
    return -3;
  if (second_thread_thread_size_res != 0)
    return -4;

  return 0;
}

int test_strncpy() {
  char *src = "test\0abcd";
  char dst[10];
  char *retval;

  char expected1[] = "test\x00\x7F\x7F\x7F\x7F\x7F";
  memset(dst, 0x7F, 10);
  retval = strncpy(dst, src, 5);
  if (retval != dst || memcmp(retval, expected1, 10))
    return 1;

  char expected2[] = "te\x7F\x7F\x7F\x7F\x7F\x7F\x7F\x7F";
  memset(dst, 0x7F, 10);
  retval = strncpy(dst, src, 2);
  if (retval != dst || memcmp(retval, expected2, 10))
    return 2;

  char expected3[] = "test\x00\x00\x00\x00\x00\x00";
  memset(dst, 0x7F, 10);
  retval = strncpy(dst, src, 10);
  if (retval != dst || memcmp(retval, expected3, 10))
    return 3;

  return 0;
}

int test_strncat() {
  {
    char uno[] = "uno\0zzzz";
    char dos[] = "dos\0ZZZZ";

    char expected[] = "unodos\0z";
    char *new = strncat(uno, dos, 100);
    if (new != uno || memcmp(new, expected, 8))
      return 1;
  }

  {
    char uno[] = "uno\0zzzz";
    char dos[] = "dos\0ZZZZ";

    char expected[] = "unod\0zzz";
    char *new = strncat(uno, dos, 1);
    if (new != uno || memcmp(new, expected, 8))
      return 2;
  }

  {
    char uno[] = "uno\0zzzz";
    char dos[] = "dosZZZZZ";

    char expected[] = "unodos\0z";
    char *new = strncat(uno, dos, 3);
    if (new != uno || memcmp(new, expected, 8))
      return 3;
  }

  return 0;
}

// Declare APIs missing from the legacy test SDK without changing that SDK.
extern size_t strnlen(const char *, size_t);
extern char *strndup(const char *, size_t);
extern void *memrchr(const void *, int, size_t);
extern char *__strncat_chk(char *, const char *, size_t, size_t);
extern void *memmem(const void *, size_t, const void *, size_t);
extern char *stpcpy(char *, const char *);
extern char *stpncpy(char *, const char *, size_t);
extern char *strchrnul(const char *, int);
extern char *strcasestr(const char *, const char *);
extern void explicit_bzero(void *, size_t);
extern int strverscmp(const char *, const char *);
extern char *strfry(char *);

int test_version_comparison_extensions() {
  int (*volatile compare)(const char *, const char *) = strverscmp;
  const char *ordered[] = {"000", "00", "01", "010", "09", "0", "1", "9", "10"};
  for (int i = 0; i < 9; i++) {
    for (int j = 0; j < 9; j++) {
      int value = compare(ordered[i], ordered[j]);
      if (((value > 0) - (value < 0)) != ((i > j) - (i < j)))
        return 1;
    }
  }
  if (compare("a", "1") <= 0 || compare("item#99", "item#100") >= 0 ||
      compare("part1_f012", "part1_f01") <= 0 || compare("0/", "01") <= 0)
    return 2;
  char long_number[513], longer[514];
  memset(long_number, '9', 512);
  long_number[512] = 0;
  memset(longer, '1', 513);
  longer[513] = 0;
  if (compare(long_number, longer) >= 0 || compare(long_number, "2") <= 0)
    return 3;
  char a[] = {(char)0xff, '9', 0}, b[] = {(char)0xff, '1', '0', 0};
  return compare(a, b) >= 0 ? 4 : 0;
}

int test_shuffle_extensions() {
  char *(*volatile shuffle)(char *) = strfry;
  char empty[] = {0, 'Z'}, one[] = {'x', 0, 'Z'};
  errno = EDOM;
  if (shuffle(empty) != empty || empty[0] || empty[1] != 'Z' ||
      shuffle(one) != one || memcmp(one, "x\0Z", 3) || errno != EDOM)
    return 1;
  unsigned char guarded[258];
  guarded[0] = guarded[257] = 'Z';
  for (int i = 1; i <= 255; i++)
    guarded[i] = (unsigned char)i;
  guarded[256] = 0;
  for (int run = 0; run < 32; run++) {
    unsigned counts[256] = {0};
    if (shuffle((char *)guarded + 1) != (char *)guarded + 1 || errno != EDOM ||
        guarded[0] != 'Z' || guarded[257] != 'Z' || guarded[256])
      return 2;
    for (int i = 1; i <= 255; i++)
      counts[guarded[i]]++;
    if (counts[0])
      return 3;
    for (int i = 1; i <= 255; i++)
      if (counts[i] != 1)
        return 4;
  }
  srand(42);
  int expected = rand();
  srand(42);
  if (!shuffle((char *)guarded + 1) || rand() != expected)
    return 5;
  return 0;
}

int test_reentrant_error_message_extensions() {
  int (*volatile describe)(int, char *, size_t) = strerror_r;
  char buf[128];
  errno = EDOM;
  if (describe(EINVAL, buf, sizeof(buf)) || strcmp(buf, "Invalid argument") ||
      strcmp(buf, strerror(EINVAL)) || errno != EDOM)
    return 1;
  memset(buf, 'Z', sizeof(buf));
  if (describe(EINVAL, buf, 4) != ERANGE || memcmp(buf, "Inv\0ZZ", 6) ||
      errno != EDOM)
    return 2;
  if (describe(EINVAL, NULL, 0) != ERANGE || describe(-1, NULL, 0) != EINVAL ||
      errno != EDOM)
    return 3;
  if (describe(-1, buf, sizeof(buf)) != EINVAL ||
      strcmp(buf, "Unknown error: -1") || describe(-1, buf, 4) != EINVAL ||
      memcmp(buf, "Unk\0", 4) || errno != EDOM)
    return 4;
  if (describe(0, buf, sizeof(buf)) != EINVAL ||
      strcmp(buf, "Unknown error: 0"))
    return 5;
  buf[0] = buf[1] = 'Z';
  if (describe(EINVAL, buf, 1) != ERANGE || buf[0] || buf[1] != 'Z')
    return 6;
  if (errno != EDOM)
    return 7;
  // Historical Darwin warns for unknown strerror values, including cache hits.
  for (int i = 0; i < 2; i++) {
    errno = EDOM;
    if (strcmp(strerror(0), "Unknown error: 0") || errno != EINVAL)
      return 8;
    errno = EDOM;
    if (strcmp(strerror(-1), "Unknown error: -1") || errno != EINVAL)
      return 9;
  }
  return 0;
}

static sem_t uf_error_message_go, uf_error_message_done;

static void *uf_error_message_worker(void *unused) {
  (void)unused;
  char buf[64];
  errno = EIO;
  int failed = 0;
  for (int i = 0; i < 16; i++) {
    if (sem_wait(&uf_error_message_go))
      return (void *)2;
    if (strerror_r(ENOENT, buf, sizeof(buf)) ||
        strcmp(buf, "No such file or directory") || errno != EIO)
      failed = 1;
    if (sem_post(&uf_error_message_done))
      return (void *)3;
  }
  return (void *)(uintptr_t)failed;
}

int test_reentrant_error_messages_across_threads() {
  pthread_t thread;
  void *result = (void *)99;
  char buf[64];
  if (sem_init(&uf_error_message_go, 0, 0) ||
      sem_init(&uf_error_message_done, 0, 0) ||
      pthread_create(&thread, NULL, uf_error_message_worker, NULL))
    return 1;
  errno = EDOM;
  int failed = 0;
  for (int i = 0; i < 16; i++) {
    if (sem_post(&uf_error_message_go) || sem_wait(&uf_error_message_done))
      return 2;
    if (strerror_r(EINVAL, buf, sizeof(buf)) ||
        strcmp(buf, "Invalid argument") || errno != EDOM)
      failed = 1;
  }
  if (pthread_join(thread, &result) || result || failed || errno != EDOM ||
      sem_destroy(&uf_error_message_go) || sem_destroy(&uf_error_message_done))
    return 3;
  // UF's completed usleep has no error to replace the caller's errno with.
  return usleep(0) || errno != EDOM ? 4 : 0;
}

int test_binary_substring_extensions() {
  void *(*volatile find)(const void *, size_t, const void *, size_t) = memmem;
  char binary[] = {'x', 0, 'a', 'b', 0, 'a', 'b'};
  char needle[] = {0, 'a', 'b'};
  if (find(binary, 7, needle, 3) != binary + 1 ||
      find(binary + 2, 5, binary + 4, 3) != binary + 4 ||
      find(binary, 7, "bad", 3) || find(NULL, 0, NULL, 1) ||
      find(binary, 7, NULL, 0) != binary || find(NULL, 0, NULL, 0))
    return 1;
  char repeated[513], pattern[129];
  memset(repeated, 'a', sizeof(repeated));
  memset(pattern, 'a', sizeof(pattern));
  repeated[512] = pattern[128] = 'b';
  return find(repeated, 513, pattern, 129) != repeated + 384 ? 2 : 0;
}

int test_end_pointer_copy_extensions() {
  char *(*volatile copy)(char *, const char *) = stpcpy;
  char *(*volatile bounded)(char *, const char *, size_t) = stpncpy;
  char dest[12];
  memset(dest, 'Z', sizeof(dest));
  if (copy(dest, "a") != dest + 1 || memcmp(dest, "a\0ZZ", 4))
    return 1;
  if (copy(dest, "") != dest || dest[0] || dest[2] != 'Z')
    return 2;
  if (copy(copy(dest, "ab"), "cd") != dest + 4 || strcmp(dest, "abcd"))
    return 3;
  char raw[] = {'a', 'b', 'c', 'd'};
  memset(dest, 'Z', sizeof(dest));
  if (bounded(dest, raw, 4) != dest + 4 || memcmp(dest, "abcdZZ", 6))
    return 4;
  if (bounded(dest, "xy", 6) != dest + 2 || memcmp(dest, "xy\0\0\0\0ZZ", 8))
    return 5;
  return bounded(NULL, NULL, 0) ? 6 : 0;
}

int test_case_and_terminator_search_extensions() {
  char *(*volatile terminator)(const char *, int) = strchrnul;
  char *(*volatile folded)(const char *, const char *) = strcasestr;
  char text[] = "aBcABc";
  if (terminator(text, 'B' + 256) != text + 1 ||
      terminator(text, 'z') != text + 6 || terminator(text, 0) != text + 6)
    return 1;
  if (folded(text, "bCa") != text + 1 || folded(text, "") != text ||
      folded(text, "too long") || folded(text, "xyz"))
    return 2;
  char bytes[] = {(char)0xe9, 'A', 'b', 0};
  char query[] = {(char)0xe9, 'a', 0};
  return folded(bytes, query) != bytes ? 3 : 0;
}

int test_overlap_copy_and_explicit_wipe_extensions() {
  void (*volatile move)(const void *, void *, size_t) = bcopy;
  void (*volatile wipe)(void *, size_t) = explicit_bzero;
  char buffer[10] = {'a', 'b', 'c', 'd', 'e', 'f', 'g', 0, 'Z', 'Z'};
  move(buffer, buffer + 2, 5);
  if (memcmp(buffer, "ababcde\0ZZ", 10))
    return 1;
  move(buffer + 2, buffer, 5);
  if (memcmp(buffer, "abcdede\0ZZ", 10))
    return 2;
  wipe(buffer + 1, 4);
  if (memcmp(buffer, "a\0\0\0\0de\0ZZ", 10))
    return 3;
  move(NULL, NULL, 0);
  wipe(NULL, 0);
  return 0;
}

int test_bounded_string_extensions() {
  char raw[3] = {'a', 'b', 'c'};
  size_t (*volatile scan)(const char *, size_t) = strnlen;
  char *(*volatile duplicate)(const char *, size_t) = strndup;
  if (scan(raw, 3) != 3 || scan(NULL, 0) != 0)
    return 1;
  char *copy = duplicate(raw, 3);
  if (!copy || strcmp(copy, "abc"))
    return 2;
  raw[0] = 'z';
  if (copy[0] != 'a')
    return 3;
  free(copy);
  copy = duplicate(NULL, 0);
  if (!copy || copy[0])
    return 4;
  free(copy);
  char dest[8] = "ab";
  if (strlcat(dest, "cdefgh", 6) != 8 || strcmp(dest, "abcde"))
    return 5;
  char unterminated[4] = {'a', 'b', 'c', 'd'};
  if (strlcat(unterminated, "xy", 4) != 6 || memcmp(unterminated, "abcd", 4))
    return 6;
  if (strlcat(NULL, "abc", 0) != 3 || strlcpy(NULL, "abc", 0) != 3)
    return 7;
  char guarded[8] = {'a', 'b', 0, 'Z', 'Z', 'Z', 'Z', 'Z'};
  char *(*volatile checked)(char *, const char *, size_t, size_t) =
      __strncat_chk;
  if (checked(guarded, "xyz", 3, 6) != guarded ||
      memcmp(guarded, "abxyz\0ZZ", 8))
    return 8;
  return 0;
}

int test_reentrant_tokenizer_extensions() {
  char text[] = ",,a:b;;c,", other[] = "x|y", global[] = "g/h";
  char *save = NULL, *other_save = NULL;
  if (strcmp(strtok(global, "/"), "g"))
    return 1;
  if (strcmp(strtok_r(text, ",:;", &save), "a"))
    return 2;
  if (strcmp(strtok_r(other, "|", &other_save), "x"))
    return 3;
  if (strcmp(strtok_r(NULL, ",:;", &save), "b"))
    return 4;
  if (strcmp(strtok_r(NULL, "|", &other_save), "y"))
    return 5;
  if (strcmp(strtok(NULL, "/"), "h"))
    return 6;
  if (strcmp(strtok_r(NULL, ",:;", &save), "c"))
    return 7;
  if (strtok_r(NULL, ",:;", &save) || strtok_r(NULL, ",:;", &save) ||
      strtok(NULL, "/") || strtok(NULL, "/"))
    return 8;
  char no_delimiter[] = "keep all";
  if (strcmp(strtok_r(no_delimiter, "", &save), "keep all") ||
      strtok_r(NULL, "", &save))
    return 9;
  return 0;
}

int test_reverse_search_and_index_extensions() {
  char bytes[] = {'a', 0, 'b', 'a'};
  if (memrchr(bytes, 'a', 4) != bytes + 3 ||
      memrchr(bytes, 256, 4) != bytes + 1 || memrchr(bytes, 'z', 4) ||
      memrchr(NULL, 0, 0))
    return 1;
  char string[] = "aba";
  if (index(string, 'a') != string || index(string, 0) != string + 3 ||
      index(string, 'z'))
    return 2;
  return 0;
}

int TestApp_fortify_failure_main(const char *mode) {
  char buffer[8] = {'a', 'b', 0, 'Z', 'Z', 'Z', 'Z', 'Z'};
  size_t capacity = 4;
  if (!strcmp(mode, "--fortify-unterminated")) {
    buffer[2] = 'c';
    capacity = 3;
  } else if (!strcmp(mode, "--fortify-zero")) {
    capacity = 0;
  }
  char *(*volatile checked)(char *, const char *, size_t, size_t) =
      __strncat_chk;
  checked(buffer, "xyz", 3, capacity);
  puts("FORTIFY_RETURNED_UNEXPECTEDLY");
  return 1;
}

int test_strlcpy() {
  {
    char src[7] = "origen";
    char dst[15] = "destinodestino";
    char expected[] = "or\0tinodestino";
    int ret = strlcpy(dst, src, 3);
    if (ret != 6 || memcmp(dst, expected, 15)) {
      printf("%d %s\t", ret, dst);
      return 1;
    }
  }

  {
    char src[7] = "origen";
    char dst[15] = "destinodestino";
    char expected[] = "orige\0odestino";
    int ret = strlcpy(dst, src, 6);
    if (ret != 6 || memcmp(dst, expected, 15)) {
      printf("%d %s\t", ret, dst);
      return 2;
    }
  }

  {
    char src[7] = "origen";
    char dst[15] = "destinodestino";
    char expected[] = "origen\0destino";
    int ret = strlcpy(dst, src, 9);
    if (ret != 6 || memcmp(dst, expected, 15)) {
      printf("%d %s\t", ret, dst);
      return 3;
    }
  }

  return 0;
}

int test_setlocale() {
  char *locale;

  // Test getting default locale
  locale = setlocale(LC_ALL, NULL);
  if (strcmp(locale, "C") != 0) {
    return 1;
  }

  // Test setting a locale category
  locale = setlocale(LC_NUMERIC, "POSIX");
  if (strcmp(locale, "POSIX") != 0) {
    return 2;
  }

  // Test if other categories are unaffected
  locale = setlocale(LC_TIME, NULL);
  if (strcmp(locale, "C") != 0) {
    return 3;
  }

  // Set C locale back for numeric
  locale = setlocale(LC_NUMERIC, "C");
  if (strcmp(locale, "C") != 0) {
    return 4;
  }

  return 0;
}

const int PATH_BUF_SIZE = 256;
// static array for path: not great, not terrible
char path[PATH_BUF_SIZE];

const char *path_test_app() {
#ifdef DEFINE_ME_WHEN_BUILDING_ON_MACOS
  // assume project dir as cwd
  return "./tests/TestApp.app";
#else
  bzero(path, PATH_BUF_SIZE);
  CFBundleRef mainBundle = CFBundleGetMainBundle();
  CFURLRef bundleURL = CFBundleCopyBundleURL(mainBundle);
  CFURLGetFileSystemRepresentation(bundleURL,
                                   true, // Resolve against base (absolute path)
                                   (UInt8 *)path, // Output buffer
                                   PATH_BUF_SIZE  // Buffer size
  );
  CFRelease(bundleURL);
  return path;
#endif
}

int test_dirent() {
  struct dirent *dp;
  DIR *dirp = opendir(path_test_app());
  if (dirp == NULL) {
    return -1;
  }
  char *contents[] = {"TestApp", "Info.plist", "PkgInfo"};
  int counts[] = {1, 1, 1};
  int total = sizeof(contents) / sizeof(char *);
  while ((dp = readdir(dirp)) != NULL) {
    for (int i = 0; i < total; i++) {
      if (strcmp(contents[i], dp->d_name) == 0) {
        counts[i]--;
        break;
      }
    }
  }
  closedir(dirp);
  for (int i = 0; i < total; i++) {
    if (counts[i] != 0) {
      return -2;
    }
  }
  return 0;
}

int test_scandir() {
  struct dirent **namelist;
  int n = scandir(path_test_app(), &namelist, NULL, NULL);
  if (n < 0) {
    return -1;
  }
  char *contents[] = {"TestApp", "Info.plist", "PkgInfo"};
  int counts[] = {1, 1, 1};
  int total = sizeof(contents) / sizeof(char *);
  while (n--) {
    for (int i = 0; i < total; i++) {
      if (strcmp(contents[i], namelist[n]->d_name) == 0) {
        counts[i]--;
        break;
      }
    }
    free(namelist[n]);
  }
  free(namelist);
  for (int i = 0; i < total; i++) {
    if (counts[i] != 0) {
      return -2;
    }
  }
  return 0;
}

int test_read_directory_as_fd() {
  FILE *dir_stream = fopen(path_test_app(), "r");
  if (dir_stream == NULL) {
    return -1;
  }
  char buffer[1024];
  size_t bytes_read = fread(buffer, 1, 4, dir_stream);
  if (bytes_read != 0) {
    return -2;
  }
  if (errno != EISDIR) {
    return -3;
  }
  fclose(dir_stream);
  return 0;
}

int test_glob() {
  glob_t gstruct;
  char buffer[256];
  bzero(buffer, 256);
  strcpy(buffer, path_test_app());
  strcat(buffer, "/*.plist");
  int res = glob(buffer, GLOB_NOSORT | GLOB_NOESCAPE, NULL, &gstruct);
  if (res != 0) {
    return -1;
  }
  char *expected = "Info.plist";
  char *found = *gstruct.gl_pathv;
  size_t found_len = strlen(found);
  size_t expected_len = strlen(expected);
  if (memcmp(found + (found_len - expected_len), expected, expected_len) != 0) {
    return -2;
  }
  globfree(&gstruct);
  bzero(buffer, 256);
  strcpy(buffer, path_test_app());
  strcat(buffer, "/*.abcdef");
  res = glob(buffer, GLOB_NOSORT | GLOB_NOESCAPE, NULL, &gstruct);
  if (res == 0) {
    return -3;
  }
  globfree(&gstruct);
  return 0;
}

int test_fnmatch() {
  // Literal match
  if (fnmatch("hello", "hello", 0) != 0)
    return -1;
  if (fnmatch("hello", "world", 0) != FNM_NOMATCH)
    return -2;

  // `*` wildcard
  if (fnmatch("*", "anything", 0) != 0)
    return -3;
  if (fnmatch("*", "", 0) != 0)
    return -4;
  if (fnmatch("hello*", "hello world", 0) != 0)
    return -5;
  if (fnmatch("*world", "hello world", 0) != 0)
    return -6;
  if (fnmatch("hel*lo", "hello", 0) != 0)
    return -7;
  if (fnmatch("hel*lo", "helxxxlo", 0) != 0)
    return -8;
  if (fnmatch("*.txt", "file.txt", 0) != 0)
    return -9;
  if (fnmatch("*.txt", "file.md", 0) != FNM_NOMATCH)
    return -10;
  return 0;
}

int test_strchr() {
  char *src = "abc";
  if (strchr(src, 'a')[0] != 'a' || strrchr(src, 'a')[0] != 'a')
    return -1;
  if (strchr(src, 'b')[0] != 'b' || strrchr(src, 'b')[0] != 'b')
    return -2;
  if (strchr(src, 'c')[0] != 'c' || strrchr(src, 'c')[0] != 'c')
    return -3;
  if (strchr(src, '\0')[0] != '\0' || strrchr(src, '\0')[0] != '\0')
    return -4;
  if (strchr(src, 'd') != NULL || strrchr(src, 'd') != NULL)
    return -5;
  return 0;
}

int test_swprintf() {
  wchar_t wcsbuf[20];
  int res = swprintf(wcsbuf, 20, L"%s", "abc");
  if (res != 3)
    return -1;
  res = swprintf(wcsbuf, 2, L"%d", 510);
  if (res != -1)
    return -2;
  res = swprintf(wcsbuf, 20, L"%S", L"abc");
  if (res != 3)
    return -3;
  return 0;
}

int test_realpath() {
  char buf[256];
  if (chdir(path_test_app()))
    return -1;
  // absolute path
  char *res = realpath("/usr", buf);
  if (!res || strcmp(res, "/usr") != 0)
    return -2;
  // relative path
  res = realpath("TestApp", buf);
  char *cwd = getcwd(NULL, 0);
  if (!res || strncmp(cwd, res, strlen(cwd)) != 0 ||
      strncmp("/TestApp", res + strlen(cwd), 8) != 0)
    return -3;
  // `..` and `.` resolution
  res = realpath("../TestApp.app/./TestApp", buf);
  if (!res || strncmp(cwd, res, strlen(cwd)) != 0 ||
      strncmp("/TestApp", res + strlen(cwd), 8) != 0)
    return -4;
  return 0;
}

int test_ungetc() {
  FILE *file = fopen("test_ungetc", "r");
  if (file == NULL) {
    return -1;
  }
  char c = getc(file);
  if (c != 'a') {
    fclose(file);
    return -2;
  }
  // ungetc with _wrong_ char
  c = ungetc('b', file);
  if (c != 'b') {
    fclose(file);
    return -3;
  }
  char buf[4];
  memset(buf, '\0', 4);
  size_t read = fread(buf, 1, 3, file);
  fclose(file);
  if (read != 3) {
    return -4;
  }
  if (strcmp(buf, "baa") != 0) {
    return -5;
  }
  return 0;
}

int test_fscanf() {
  char str[256];
  int a;
  float f;
  FILE *file = fopen("test_fscanf", "r");
  if (file == NULL) {
    return -1;
  }
  int matched = fscanf(file, "%s", str);
  if (!(matched == 1 && strcmp(str, "no_spaces_line") == 0)) {
    return -2;
  }
  matched = fscanf(file, "%s %d", str, &a);
  if (!(matched == 2 && strcmp(str, "one") == 0 && a == -100)) {
    return -3;
  }
  matched = fscanf(file, "%s", str);
  if (!(matched == 1 && strcmp(str, "string") == 0)) {
    return -4;
  }
  matched = fscanf(file, "%f", &f);
  if (!(matched == 1 && fabs(f - 3.14) < 0.001)) {
    return -5;
  }
  matched = fscanf(file, "%s", str);
  if (matched != -1) { // EOF
    return -6;
  }
  fclose(file);
  return 0;
}

// Below tests are on par with test_sscanf(),
// but reading data from a file instead.
// Please update those as well if you add new
// test cases to test_sscanf()
int test_fscanf_new() {
  FILE *file = fopen("test_fscanf_new", "r");
  if (!file)
    return -1;

#define SKIP_LINE(f)                                                           \
  do {                                                                         \
    int ch;                                                                    \
    while ((ch = fgetc(f)) != '\n' && ch != -1)                                \
      ;                                                                        \
  } while (0)

  int a, b, matched;
  short c, d;
  float f, f1, f2, f3, f4, f5, f6;
  double lf;
  char str[256], str1[4];

  matched = fscanf(file, "%d.%d", &a, &b);
  if (!(matched == 2 && a == 1 && b == 23))
    return -2;
  SKIP_LINE(file);

  matched = fscanf(file, "abc%d.%d", &a, &b);
  if (!(matched == 2 && a == 111 && b == 42))
    return -3;
  SKIP_LINE(file);

  matched = fscanf(file, "%d.%d", &a, &b);
  if (matched != 0)
    return -4;
  SKIP_LINE(file);

  matched = fscanf(file, "%[^,],%d", str, &b);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && b == 8))
    return -5;
  SKIP_LINE(file);

  matched = fscanf(file, "%hi,%i", &c, &a);
  if (!(matched == 2 && c == 9 && a == 10))
    return -6;
  SKIP_LINE(file);

  matched = fscanf(file, "%d", &a);
  if (matched != 0)
    return -7;
  SKIP_LINE(file);

  matched = fscanf(file, "%d %d", &a, &b);
  if (!(matched == 2 && a == 10 && b == -10))
    return -8;
  SKIP_LINE(file);

  matched = fscanf(file, "%hd %hd", &c, &d);
  if (!(matched == 2 && c == 10 && d == -10))
    return -9;
  SKIP_LINE(file);

  matched = fscanf(file, "%d %d", &a, &b);
  if (!(matched == 1 && a == 3000))
    return -10;
  SKIP_LINE(file);

  matched = fscanf(file, "%08x", &a);
  if (!(matched == 1 && a == 16711680))
    return -11;
  SKIP_LINE(file);

  matched = fscanf(file, "%s %f", str, &f);
  if (!(matched == 2 && strcmp(str, "ABC") == 0 && f == 1.0f))
    return -12;
  SKIP_LINE(file);

  matched = fscanf(file, "%s\t%f", str, &f);
  if (!(matched == 2 && strcmp(str, "ABC") == 0 && f == 1.0f))
    return -13;
  SKIP_LINE(file);

  matched = fscanf(file, "%s %f", str, &f);
  if (!(matched == 2 && strcmp(str, "MAX") == 0 && f == 48.0f))
    return -14;
  SKIP_LINE(file);

  matched = fscanf(file, "%i", &a);
  if (!(matched == 1 && a == 9))
    return -15;
  SKIP_LINE(file);

  matched = fscanf(file, "%i", &a);
  if (!(matched == 1 && a == 0))
    return -16;
  SKIP_LINE(file);

  matched = fscanf(file, "%2x%2x", &a, &b);
  if (!(matched == 2 && a == 0xFF && b == 0x00))
    return -17;
  SKIP_LINE(file);

  matched = fscanf(file, "%10x", &a);
  if (!(matched == 1 && a == 0xAA))
    return -18;
  SKIP_LINE(file);

  matched = fscanf(file, "%lf", &lf);
  if (!(matched == 1 && lf == 3.14159265359))
    return -19;
  SKIP_LINE(file);

  matched = fscanf(file, "%[a-z]", str);
  if (!(matched == 1 && strcmp(str, "hello") == 0))
    return -20;
  SKIP_LINE(file);

  matched = fscanf(file, "%[^0-9]", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -21;
  SKIP_LINE(file);

  matched = fscanf(file, "%[-0-9]", str);
  if (!(matched == 1 && strcmp(str, "-123") == 0))
    return -22;
  SKIP_LINE(file);

  matched = fscanf(file, "%[a-z-]", str);
  if (!(matched == 1 && strcmp(str, "a-b") == 0))
    return -23;
  SKIP_LINE(file);

  matched = fscanf(file, "%[^0-9]", str);
  if (matched != 0)
    return -24;
  SKIP_LINE(file);

  matched = fscanf(file, "%[A-Za-z0-9_]", str);
  if (!(matched == 1 && strcmp(str, "Var_123") == 0))
    return -25;
  SKIP_LINE(file);

  matched = fscanf(file, "%s", str);
  if (!(matched == 1 && strcmp(str, "NAME") == 0))
    return -26;
  SKIP_LINE(file);

  matched = fscanf(file, "%s", str);
  if (!(matched == 1 && strcmp(str, "NAME") == 0))
    return -27;
  SKIP_LINE(file);

  matched = fscanf(file, "%s %s", str, str1);
  if (!(matched == 2 && strcmp(str, "A") == 0 && strcmp(str1, "B") == 0))
    return -28;
  SKIP_LINE(file);

  matched = fscanf(file, " numJoints %d", &a);
  if (!(matched == 1 && a == 110))
    return -29;
  SKIP_LINE(file);

  matched = fscanf(file, " %s %d ( %f %f %f ) ( %f %f %f )", str, &a, &f1, &f2,
                   &f3, &f4, &f5, &f6);
  if (!(matched == 8 && strcmp(str, "\"origin\"") == 0 && a == -1 &&
        f1 == 0.0f && fabs(f4 + 0.7071067095f) < 1e-10f && f6 == 0.0f))
    return -30;
  SKIP_LINE(file);

  // '%g' test cases
  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && f == 123.0f))
    return -31;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -32;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23e-4f) < 1e-8f))
    return -33;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 12300.0f) < 1e-5f))
    return -34;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -35;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - -1.23f) < 1e-5f))
    return -36;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 0.5f) < 1e-5f))
    return -37;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - -0.5f) < 1e-5f))
    return -38;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 100000.0f) < 1e-5f))
    return -39;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 100000.0f) < 1e-5f))
    return -40;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 1.23f) < 1e-5f))
    return -41;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - 12300.0f) < 1e-5f))
    return -42;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && fabs(f - -0.000123f) < 1e-8f))
    return -43;
  SKIP_LINE(file);

  matched = fscanf(file, "%g", &f);
  if (!(matched == 1 && f == 123.0f))
    return -44;
  SKIP_LINE(file);

  // %hu (unsigned short) edge cases
  unsigned short us, us2;
  matched = fscanf(file, "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -45;
  SKIP_LINE(file);

  matched = fscanf(file, "%hu", &us);
  if (!(matched == 1 && us == 65535))
    return -46;
  SKIP_LINE(file);

  // Truncation: 65536 wraps to 0 as unsigned short
  matched = fscanf(file, "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -47;
  SKIP_LINE(file);

  // Truncation: 65537 wraps to 1 as unsigned short
  matched = fscanf(file, "%hu", &us);
  if (!(matched == 1 && us == 1))
    return -48;
  SKIP_LINE(file);

  matched = fscanf(file, "%hu %hu", &us, &us2);
  if (!(matched == 2 && us == 100 && us2 == 200))
    return -49;
  SKIP_LINE(file);

  // width limits the conversion
  matched = fscanf(file, "%3hu", &us);
  if (!(matched == 1 && us == 123))
    return -50;
  SKIP_LINE(file);

  // %hhu (unsigned char) edge cases
  unsigned char uc, uc2;
  matched = fscanf(file, "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -51;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhu", &uc);
  if (!(matched == 1 && uc == 255))
    return -52;
  SKIP_LINE(file);

  // Truncation: 256 wraps to 0 as unsigned char
  matched = fscanf(file, "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -53;
  SKIP_LINE(file);

  // Truncation: 257 wraps to 1 as unsigned char
  matched = fscanf(file, "%hhu", &uc);
  if (!(matched == 1 && uc == 1))
    return -54;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhu %hhu", &uc, &uc2);
  if (!(matched == 2 && uc == 10 && uc2 == 20))
    return -55;
  SKIP_LINE(file);

  // width limits the conversion
  matched = fscanf(file, "%2hhu", &uc);
  if (!(matched == 1 && uc == 12))
    return -56;
  SKIP_LINE(file);

  // Overflow above UINT_MAX: per C semantics, the input is parsed as
  // a wide unsigned and only the low bits are stored, so 0x100000000
  // gives 0 in both u16 and u8.
  matched = fscanf(file, "%hu", &us);
  if (!(matched == 1 && us == 0))
    return -57;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhu", &uc);
  if (!(matched == 1 && uc == 0))
    return -58;
  SKIP_LINE(file);

  // %hx (unsigned short, hex) truncation
  matched = fscanf(file, "%hx", &us);
  if (!(matched == 1 && us == 0xFFFF))
    return -59;
  SKIP_LINE(file);

  // Truncation: 0x10000 wraps to 0 as unsigned short
  matched = fscanf(file, "%hx", &us);
  if (!(matched == 1 && us == 0))
    return -60;
  SKIP_LINE(file);

  // Truncation: 0x10001 wraps to 1 as unsigned short
  matched = fscanf(file, "%hx", &us);
  if (!(matched == 1 && us == 1))
    return -61;
  SKIP_LINE(file);

  // %hhx (unsigned char, hex) truncation
  matched = fscanf(file, "%hhx", &uc);
  if (!(matched == 1 && uc == 0xFF))
    return -62;
  SKIP_LINE(file);

  // Truncation: 0x100 wraps to 0 as unsigned char
  matched = fscanf(file, "%hhx", &uc);
  if (!(matched == 1 && uc == 0))
    return -63;
  SKIP_LINE(file);

  // Truncation: 0x101 wraps to 1 as unsigned char
  matched = fscanf(file, "%hhx", &uc);
  if (!(matched == 1 && uc == 1))
    return -64;
  SKIP_LINE(file);

  // %hd (signed short) edge cases
  short ss, ss2;
  matched = fscanf(file, "%hd", &ss);
  if (!(matched == 1 && ss == 0))
    return -65;
  SKIP_LINE(file);

  matched = fscanf(file, "%hd", &ss);
  if (!(matched == 1 && ss == 32767))
    return -66;
  SKIP_LINE(file);

  matched = fscanf(file, "%hd", &ss);
  if (!(matched == 1 && ss == -32768))
    return -67;
  SKIP_LINE(file);

  // Truncation: 32768 wraps to -32768 as signed short
  matched = fscanf(file, "%hd", &ss);
  if (!(matched == 1 && ss == -32768))
    return -68;
  SKIP_LINE(file);

  // Truncation: -32769 wraps to 32767 as signed short
  matched = fscanf(file, "%hd", &ss);
  if (!(matched == 1 && ss == 32767))
    return -69;
  SKIP_LINE(file);

  matched = fscanf(file, "%hd %hd", &ss, &ss2);
  if (!(matched == 2 && ss == -100 && ss2 == 200))
    return -70;
  SKIP_LINE(file);

  // width limits the conversion
  matched = fscanf(file, "%3hd", &ss);
  if (!(matched == 1 && ss == 123))
    return -71;
  SKIP_LINE(file);

  // width counts the sign character
  matched = fscanf(file, "%4hd", &ss);
  if (!(matched == 1 && ss == -123))
    return -72;
  SKIP_LINE(file);

  // %hhd (signed char) edge cases
  signed char sc, sc2;
  matched = fscanf(file, "%hhd", &sc);
  if (!(matched == 1 && sc == 0))
    return -73;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhd", &sc);
  if (!(matched == 1 && sc == 127))
    return -74;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhd", &sc);
  if (!(matched == 1 && sc == -128))
    return -75;
  SKIP_LINE(file);

  // Truncation: 128 wraps to -128 as signed char
  matched = fscanf(file, "%hhd", &sc);
  if (!(matched == 1 && sc == -128))
    return -76;
  SKIP_LINE(file);

  // Truncation: -129 wraps to 127 as signed char
  matched = fscanf(file, "%hhd", &sc);
  if (!(matched == 1 && sc == 127))
    return -77;
  SKIP_LINE(file);

  matched = fscanf(file, "%hhd %hhd", &sc, &sc2);
  if (!(matched == 2 && sc == -10 && sc2 == 20))
    return -78;
  SKIP_LINE(file);

  // width limits the conversion
  matched = fscanf(file, "%2hhd", &sc);
  if (!(matched == 1 && sc == 12))
    return -79;
  SKIP_LINE(file);

  // width counts the sign character
  matched = fscanf(file, "%3hhd", &sc);
  if (!(matched == 1 && sc == -12))
    return -80;
  SKIP_LINE(file);

  // max_width for %s specifier
  // width truncates a longer token
  matched = fscanf(file, "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -81;
  SKIP_LINE(file);

  // width larger than the token reads the whole token
  matched = fscanf(file, "%5s", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -82;
  SKIP_LINE(file);

  // width of 1
  matched = fscanf(file, "%1s", str);
  if (!(matched == 1 && strcmp(str, "a") == 0))
    return -83;
  SKIP_LINE(file);

  // input length exactly equals width
  matched = fscanf(file, "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -84;
  SKIP_LINE(file);

  // %s stopped by whitespace before width is reached
  matched = fscanf(file, "%5s", str);
  if (!(matched == 1 && strcmp(str, "ab") == 0))
    return -85;
  SKIP_LINE(file);

  // leading whitespace is skipped and does not count towards width
  matched = fscanf(file, "%3s", str);
  if (!(matched == 1 && strcmp(str, "abc") == 0))
    return -86;
  SKIP_LINE(file);

  // width limits %s leaving remainder for next conversion
  matched = fscanf(file, "%3s%s", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -87;
  SKIP_LINE(file);

  // two width-limited %s split a single token
  matched = fscanf(file, "%3s%3s", str, str1);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && strcmp(str1, "def") == 0))
    return -88;
  SKIP_LINE(file);

  // width-truncated %s leaves the remainder for a following %d
  matched = fscanf(file, "%3s%d", str, &a);
  if (!(matched == 2 && strcmp(str, "abc") == 0 && a == 123))
    return -89;
  SKIP_LINE(file);

  fclose(file);
  return 0;
}

// P2G attempts this image layout when making a resume snapshot. Returning
// NULL must let its existing failure handling run, rather than aborting.
int test_CGImage_invalid_rgb32() {
  const unsigned char pixels[] = {32, 16, 8, 64, 5, 10, 15, 255};
  CFDataRef data = CFDataCreate(NULL, pixels, sizeof(pixels));
  CGDataProviderRef provider = CGDataProviderCreateWithCFData(data);
  CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
  CGImageRef invalid =
      CGImageCreate(2, 1, 8, 32, 8, space, 0 /* kCGImageAlphaNone */, provider,
                    NULL, false, 0);
  if (invalid != NULL) {
    CGImageRelease(invalid);
    CFRelease(provider);
    CFRelease(data);
    CGColorSpaceRelease(space);
    return -1;
  }
  // The rejected call must not consume the provider or affect valid images.
  CGImageRef valid = CGImageCreate(2, 1, 8, 32, 8, space,
                                   1 /* kCGImageAlphaPremultipliedLast */,
                                   provider, NULL, false, 0);
  int result = 0;
  if (valid == NULL) {
    result = -2;
  } else {
    CFDataRef raw = CGDataProviderCopyData(CGImageGetDataProvider(valid));
    if (CGImageGetWidth(valid) != 2 || CGImageGetHeight(valid) != 1 ||
        CFDataGetLength(raw) != sizeof(pixels) ||
        memcmp(CFDataGetBytePtr(raw), pixels, sizeof(pixels)) != 0) {
      result = -3;
    }
    CFRelease(raw);
    CGImageRelease(valid);
  }
  CFRelease(provider);
  CFRelease(data);
  CGColorSpaceRelease(space);
  return result;
}

int test_CGImage_JPEG() {
  FILE *file = fopen("test_1x1_black_pixel.jpg", "r");
  if (file == NULL) {
    return -1;
  }
  char buf[720];
  memset(buf, '\0', 720);
  size_t read = fread(buf, 1, 720, file);
  fclose(file);
  if (read != 720) {
    return -2;
  }
  CFDataRef dataRef = CFDataCreate(NULL, buf, sizeof(buf));
  if (dataRef == NULL) {
    return -3;
  }
  CGDataProviderRef dataProvider = CGDataProviderCreateWithCFData(dataRef);
  if (dataRef == NULL) {
    return -4;
  }
  CGImageRef imageRef = CGImageCreateWithJPEGDataProvider(
      dataProvider, NULL, 1 /* true */, 0 /* kCGRenderingIntentDefault */);
  if (imageRef == NULL) {
    return -5;
  }
  size_t width = CGImageGetWidth(imageRef);
  size_t height = CGImageGetHeight(imageRef);
  if (!(width == 1 && height == 1)) {
    return -6;
  }
  CFDataRef rawData = CGDataProviderCopyData(CGImageGetDataProvider(imageRef));
  const unsigned char *bytes = CFDataGetBytePtr(rawData);
  // Check that pixel is indeed a RGB black one
  if (!(bytes[0] == 0 && bytes[1] == 0 && bytes[2] == 0)) {
    return -7;
  }
  CFRelease(rawData);
  CFRelease(imageRef);
  CFRelease(dataProvider);
  return 0;
}

int test_CFStringFind() {
  CFStringRef a =
      CFStringCreateWithCString(NULL, "/a/b/c/b", kCFStringEncodingASCII);
  CFStringRef b = CFStringCreateWithCString(NULL, "/b", kCFStringEncodingASCII);
  CFStringRef d = CFStringCreateWithCString(NULL, "/d", kCFStringEncodingASCII);
  // 0 for default options
  CFRange r = CFStringFind(a, b, 0);
  if (!(r.location == 2 && r.length == 2)) {
    return -1;
  }
  // 4 for kCFCompareBackwards
  r = CFStringFind(a, b, 4);
  if (!(r.location == 6 && r.length == 2)) {
    return -2;
  }
  // search string in itself
  r = CFStringFind(a, a, 0);
  if (!(r.location == 0 && r.length == 8)) {
    return -3;
  }
  // search string in itself, backwards
  r = CFStringFind(a, a, 4);
  if (!(r.location == 0 && r.length == 8)) {
    return -4;
  }
  // not found case
  r = CFStringFind(a, d, 0);
  if (!(r.location == -1 && r.length == 0)) {
    return -5;
  }
  // 1 for kCFCompareCaseInsensitive
  CFStringRef b2 = CFStringCreateWithCString(NULL, "/B", 0x0600);
  r = CFStringFind(a, b2, 1);
  if (!(r.location == 2 && r.length == 2)) {
    return -6;
  }
  return 0;
}

int test_strcspn() {
  size_t res = strcspn("abcdef", "abcd");
  if (res != 0) {
    return -1;
  }
  res = strcspn("abcdef", "ef");
  if (res != 4) {
    return -2;
  }
  res = strcspn("abcdef", "");
  if (res != 6) {
    return -3;
  }
  return 0;
}

int test_mbstowcs() {
  wchar_t wbuffer[64];
  char buffer[64];
  size_t res;

  char *test_str = "Hello, World!";
  res = mbstowcs(wbuffer, test_str, 64);
  if (res == (size_t)-1) {
    return -1;
  }

  res = wcstombs(buffer, wbuffer, 64);
  if (res == (size_t)-1) {
    return -2;
  }

  if (strcmp(test_str, buffer) != 0) {
    return -3;
  }

  return 0;
}

int test_CFMutableString() {
  CFMutableStringRef mut_str = CFStringCreateMutable(NULL, 0);
  CFStringRef fmt = CFStringCreateWithCString(NULL, "%d %.2f", 0x0600);
  CFStringAppendFormat(mut_str, NULL, fmt, -100, 3.14);
  CFStringRef res = CFStringCreateWithCString(NULL, "-100 3.14", 0x0600);
  if (CFStringCompare(mut_str, res, 0) != 0) {
    return -1;
  }
  return 0;
}

int test_fwrite() {
  FILE *some_file = fopen("TestApp", "r");
  size_t res = fwrite(NULL, 1, 1, some_file);
  fclose(some_file);
  if (res != 0) {
    return -1;
  }
  return 0;
}

// === flockfile / funlockfile tests ===

int test_flockfile_basic() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  flockfile(file);
  funlockfile(file);

  fclose(file);
  return 0;
}

// flockfile is required to be recursive: the same thread may acquire the
// lock multiple times and must release it the same number of times.
int test_flockfile_recursive() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  flockfile(file);
  flockfile(file);
  flockfile(file);
  funlockfile(file);
  funlockfile(file);
  funlockfile(file);

  // After unlocking the matching number of times, the stream must be
  // available again, so ftrylockfile must succeed.
  if (ftrylockfile(file) != 0) {
    fclose(file);
    return -2;
  }
  funlockfile(file);

  fclose(file);
  return 0;
}

int test_ftrylockfile_unlocked() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  if (ftrylockfile(file) != 0) {
    fclose(file);
    return -2;
  }
  funlockfile(file);

  fclose(file);
  return 0;
}

struct ftrylockfile_args {
  FILE *file;
  int result;
};

void *ftrylockfile_other_thread(void *arg) {
  struct ftrylockfile_args *a = arg;
  a->result = ftrylockfile(a->file);
  // If we somehow obtained the lock (we shouldn't), release it so the
  // main thread is not left blocked.
  if (a->result == 0)
    funlockfile(a->file);
  return NULL;
}

// When a stream is locked by one thread, ftrylockfile from another thread
// must fail (return non-zero).
int test_ftrylockfile_locked_by_other_thread() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  flockfile(file);

  struct ftrylockfile_args args = {file, -1};
  pthread_t p;
  if (pthread_create(&p, NULL, ftrylockfile_other_thread, &args) != 0) {
    funlockfile(file);
    fclose(file);
    return -2;
  }
  if (pthread_join(p, NULL) != 0) {
    funlockfile(file);
    fclose(file);
    return -3;
  }

  funlockfile(file);
  fclose(file);

  if (args.result == 0)
    return -4;
  return 0;
}

struct flockfile_blocking_args {
  FILE *file;
  pthread_mutex_t *mu;
  pthread_cond_t *cv;
  int *started;
  int *acquired;
};

void *flockfile_blocking_thread(void *arg) {
  struct flockfile_blocking_args *a = arg;

  // Announce that we are about to attempt flockfile, so the main thread
  // does not have to rely on a sleep to know we have made progress.
  pthread_mutex_lock(a->mu);
  *(a->started) = 1;
  pthread_cond_signal(a->cv);
  pthread_mutex_unlock(a->mu);

  // This call must block until the main thread releases the stream lock.
  flockfile(a->file);

  pthread_mutex_lock(a->mu);
  *(a->acquired) = 1;
  pthread_cond_signal(a->cv);
  pthread_mutex_unlock(a->mu);

  funlockfile(a->file);
  return NULL;
}

// flockfile must block another thread until the lock is released by the
// thread that currently owns it.
int test_flockfile_blocks_other_thread() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  pthread_mutex_t mu;
  if (pthread_mutex_init(&mu, NULL) != 0) {
    fclose(file);
    return -2;
  }
  pthread_cond_t cv;
  if (pthread_cond_init(&cv, NULL) != 0) {
    pthread_mutex_destroy(&mu);
    fclose(file);
    return -3;
  }

  int started = 0;
  int acquired = 0;
  struct flockfile_blocking_args args = {file, &mu, &cv, &started, &acquired};

  flockfile(file);

  pthread_t p;
  if (pthread_create(&p, NULL, flockfile_blocking_thread, &args) != 0) {
    funlockfile(file);
    pthread_cond_destroy(&cv);
    pthread_mutex_destroy(&mu);
    fclose(file);
    return -4;
  }

  // Wait until the worker thread has reached the point right before
  // flockfile, then yield so the scheduler can run it into the blocking
  // call.
  pthread_mutex_lock(&mu);
  while (!started)
    pthread_cond_wait(&cv, &mu);
  pthread_mutex_unlock(&mu);
  sched_yield();

  pthread_mutex_lock(&mu);
  int acquired_before_unlock = acquired;
  pthread_mutex_unlock(&mu);

  funlockfile(file);

  // Once we have released the stream lock, the worker must eventually be
  // able to acquire it. Wait for that signal rather than for thread join
  // so that a hang in flockfile is attributed to this assertion.
  pthread_mutex_lock(&mu);
  while (!acquired)
    pthread_cond_wait(&cv, &mu);
  pthread_mutex_unlock(&mu);

  if (pthread_join(p, NULL) != 0) {
    pthread_cond_destroy(&cv);
    pthread_mutex_destroy(&mu);
    fclose(file);
    return -5;
  }

  pthread_cond_destroy(&cv);
  pthread_mutex_destroy(&mu);
  fclose(file);

  if (acquired_before_unlock != 0)
    return -6;
  if (acquired != 1)
    return -7;
  return 0;
}

// flockfile is most commonly used to make a sequence of stdio calls atomic
// from the perspective of other threads. Verify that the main stdio entry
// points work normally while the current thread holds the stream lock.
int test_flockfile_io_while_locked() {
  FILE *file = fopen("TestApp", "r");
  if (file == NULL)
    return -1;

  flockfile(file);

  // ftello must succeed and report the initial position.
  if (ftello(file) != 0) {
    funlockfile(file);
    fclose(file);
    return -2;
  }

  // fread must succeed and advance the position.
  char buf[8];
  size_t n = fread(buf, 1, sizeof(buf), file);
  if (n != sizeof(buf)) {
    funlockfile(file);
    fclose(file);
    return -3;
  }
  if (ftello(file) != (off_t)sizeof(buf)) {
    funlockfile(file);
    fclose(file);
    return -4;
  }

  // fseeko must succeed and reset the position.
  if (fseeko(file, 0, SEEK_SET) != 0) {
    funlockfile(file);
    fclose(file);
    return -5;
  }
  if (ftello(file) != 0) {
    funlockfile(file);
    fclose(file);
    return -6;
  }

  // feof / clearerr / fflush / fileno must not abort while the lock is
  // held by the calling thread.
  (void)feof(file);
  clearerr(file);
  (void)fflush(file);
  if (fileno(file) < 0) {
    funlockfile(file);
    fclose(file);
    return -7;
  }

  funlockfile(file);
  fclose(file);
  return 0;
}

// === end flockfile / funlockfile tests ===

int test_open() {
  int fd;
  // Test opening directories
  fd = open("/usr", O_RDONLY);
  if (fd == -1) {
    return -1;
  }
  close(fd);

  fd = open("/usr", O_WRONLY);
  if (fd != -1) {
    close(fd);
    return -2;
  }

  fd = open("/usr", O_RDWR);
  if (fd != -1) {
    close(fd);
    return -3;
  }

  return 0;
}

int test_close() {
  if (close(0) != 0)
    return -1;
  if (close(-1) == 0)
    return -2;
  if (close(1000) == 0)
    return -3;
  return 0;
}

int test_CFMutableDictionary_NullCallbacks() {
  CFMutableDictionaryRef dict = CFDictionaryCreateMutable(NULL, 0, NULL, NULL);
  if (dict == NULL) {
    return -1;
  }

  const char *key = "Key";
  const char *value = "Value";
  CFDictionaryAddValue(dict, key, value);
  const void *retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != value) {
    CFRelease(dict);
    return -2;
  }

  const char *valueNew = "NewValue";
  CFDictionaryAddValue(dict, key, valueNew);
  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != value) {
    CFRelease(dict);
    return -3;
  }

  CFDictionarySetValue(dict, key, NULL);
  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != NULL) {
    CFRelease(dict);
    return -4;
  }

  CFDictionarySetValue(dict, key, valueNew);
  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != valueNew) {
    CFRelease(dict);
    return -5;
  }

  CFDictionaryRemoveValue(dict, key);
  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != NULL) {
    CFRelease(dict);
    return -6;
  }

  CFDictionaryAddValue(dict, key, value);
  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != value) {
    CFRelease(dict);
    return -7;
  }

  CFIndex count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -8;
  }

  const void **keys = malloc(sizeof(char *) * count);
  const void **values = malloc(sizeof(char *) * count);
  CFDictionaryGetKeysAndValues(dict, keys, values);
  if (keys[0] != key || values[0] != value) {
    free(keys);
    free(values);
    CFRelease(dict);
    return -9;
  }
  free(keys);
  free(values);

  CFDictionaryRemoveAllValues(dict);
  count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(dict);
    return -10;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(dict);
    return -11;
  }

  CFRelease(dict);
  return 0;
}

// Counters for checking key/value callbacks
static int keyRetainCount = 0;
static int keyReleaseCount = 0;
static int keyEqualCount = 0;
static int keyHashCount = 0;
static int valueRetainCount = 0;
static int valueReleaseCount = 0;
static int valueEqualCount = 0;

// Custom CFDictionary key/value callbacks
const void *TestKeyRetain(CFAllocatorRef allocator, const void *value) {
  keyRetainCount++;
  if (value == NULL) {
    return NULL;
  }
  return strdup((const char *)value);
}
void TestKeyRelease(CFAllocatorRef allocator, const void *value) {
  keyReleaseCount++;
  if (value == NULL) {
    return;
  }
  free((void *)value);
}
Boolean TestKeyEqual(const void *value1, const void *value2) {
  keyEqualCount++;
  if (value1 == value2) {
    return 1;
  }
  if (value1 == NULL || value2 == NULL) {
    return 0;
  }
  return strcmp((const char *)value1, (const char *)value2) == 0;
}
CFHashCode TestKeyHash(const void *value) {
  keyHashCount++;
  return (value == NULL) ? 0 : 5;
}
const void *TestValueRetain(CFAllocatorRef allocator, const void *value) {
  valueRetainCount++;
  return (value == NULL) ? NULL : strdup((const char *)value);
}
void TestValueRelease(CFAllocatorRef allocator, const void *value) {
  valueReleaseCount++;
  if (value == NULL) {
    return;
  }
  free((void *)value);
}
Boolean TestValueEqual(const void *value1, const void *value2) {
  valueEqualCount++;
  if (value1 == value2) {
    return 1;
  }
  if (value1 == NULL || value2 == NULL) {
    return 0;
  }
  return strcmp((const char *)value1, (const char *)value2) == 0;
}
CFDictionaryKeyCallBacks testKeyCallBacks = {0, // version
                                             TestKeyRetain,
                                             TestKeyRelease,
                                             NULL,
                                             TestKeyEqual,
                                             TestKeyHash};
CFDictionaryValueCallBacks testValueCallBacks = {
    0, // version
    TestValueRetain, TestValueRelease, NULL, TestValueEqual};

int test_CFMutableDictionary_CustomCallbacks_PrimitiveTypes() {
  // Reset counters
  keyRetainCount = keyReleaseCount = keyEqualCount = keyHashCount = 0;
  valueRetainCount = valueReleaseCount = valueEqualCount = 0;

  CFMutableDictionaryRef dict = CFDictionaryCreateMutable(
      NULL, 0, &testKeyCallBacks, &testValueCallBacks);
  if (dict == NULL) {
    return -1;
  }

  CFIndex count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(dict);
    return -2;
  }

  const char *key = "Key";
  const char *value = "Value";
  CFDictionaryAddValue(dict, key, value);

  // Hash key function should be called at least once
  if (keyRetainCount != 1 || keyHashCount < 1 || valueRetainCount != 1) {
    CFRelease(dict);
    return -3;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -4;
  }

  const void *retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue == NULL) {
    CFRelease(dict);
    return -5;
  }
  if (strcmp((const char *)retrievedValue, value) != 0) {
    CFRelease(dict);
    return -6;
  }
  if (keyEqualCount < 1) {
    CFRelease(dict);
    return -7;
  }

  const char *valueNew = "NewValue";
  CFDictionaryAddValue(dict, key, valueNew);
  // The key already exists, so the value should not be added
  if (keyRetainCount != 1 || valueRetainCount != 1) {
    CFRelease(dict);
    return -8;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -9;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (strcmp((const char *)retrievedValue, value) != 0) {
    CFRelease(dict);
    return -10;
  }

  CFDictionarySetValue(dict, key, NULL);
  if (valueReleaseCount != 1 || valueRetainCount != 2) {
    CFRelease(dict);
    return -11;
  }

  // Check that count is 1 after setting value to NULL
  // (NULL is a valid value for CFDictionary!)
  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -12;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != NULL) {
    CFRelease(dict);
    return -13;
  }
  if (keyReleaseCount != 1 || valueReleaseCount != 1) {
    CFRelease(dict);
    return -14;
  }

  CFDictionarySetValue(dict, key, valueNew);
  if (keyReleaseCount != 2 || valueReleaseCount != 2) {
    CFRelease(dict);
    return -15;
  }
  if (valueRetainCount != 3) {
    CFRelease(dict);
    return -16;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -17;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue == NULL ||
      strcmp((const char *)retrievedValue, valueNew) != 0) {
    CFRelease(dict);
    return -18;
  }
  if (keyReleaseCount != 2 || valueReleaseCount != 2) {
    CFRelease(dict);
    return -19;
  }

  CFDictionaryRemoveValue(dict, key);
  if (keyReleaseCount != 3 || valueReleaseCount != 3) {
    CFRelease(dict);
    return -20;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(dict);
    return -21;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != NULL) {
    CFRelease(dict);
    return -22;
  }
  if (keyRetainCount != 3 || valueRetainCount != 3) {
    CFRelease(dict);
    return -23;
  }

  CFDictionaryAddValue(dict, key, value);
  if (keyRetainCount != 4 || valueRetainCount != 4) {
    CFRelease(dict);
    return -24;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -25;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue == NULL ||
      strcmp((const char *)retrievedValue, value) != 0) {
    CFRelease(dict);
    return -26;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(dict);
    return -27;
  }

  const void **keys = malloc(sizeof(void *) * count);
  const void **values = malloc(sizeof(void *) * count);
  CFDictionaryGetKeysAndValues(dict, keys, values);
  if (strcmp((const char *)keys[0], key) != 0 ||
      strcmp((const char *)values[0], value) != 0) {
    free(keys);
    free(values);
    CFRelease(dict);
    return -28;
  }
  free(keys);
  free(values);
  if (keyReleaseCount != 3 || valueReleaseCount != 3) {
    CFRelease(dict);
    return -29;
  }

  CFDictionaryRemoveAllValues(dict);
  if (keyReleaseCount != 4 || valueReleaseCount != 4) {
    CFRelease(dict);
    return -30;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(dict);
    return -31;
  }

  // Check that value equality callback was not called (based on macOS behavior)
  if (valueEqualCount != 0) {
    CFRelease(dict);
    return -32;
  }

  CFRelease(dict);
  return 0;
}

// Counters for retain and release.
//
// We couldn't relay on the retainCounts of the objects directly
// as Objective-C retainCount method is meant to be for debug
// purposes only and modern versions are using tagged pointers anyway,
// thus return value of this method can be meaningless.
// Instead, we hook counter to the retain/release callbacks
// and check for changes in deltas
// (because actual counts could be different between implementations).
static int retainCount = 0;
static int releaseCount = 0;

// Callbacks similar to kCFTypeDictionaryKeyCallBacks and
// kCFTypeDictionaryValueCallBacks
const void *CFRetainWrapper(CFAllocatorRef allocator, const void *value) {
  retainCount++;
  return CFRetain(value);
}

void CFReleaseWrapper(CFAllocatorRef allocator, const void *value) {
  releaseCount++;
  CFRelease(value);
}
CFHashCode CFHashWrapper(const void *value) { return CFHash(value); }
Boolean CFEqualWrapper(const void *value1, const void *value2) {
  return CFEqual(value1, value2);
}
CFDictionaryKeyCallBacks testDefaultKeyCallBacks = {
    0, // version
    CFRetainWrapper,
    CFReleaseWrapper,
    NULL, // stub of CFCopyDescription
    CFEqualWrapper,
    CFHashWrapper};
CFDictionaryValueCallBacks testDefaultValueCallBacks = {
    0, // version
    CFRetainWrapper, CFReleaseWrapper,
    NULL, // stub of CFCopyDescription
    CFEqualWrapper};

int test_CFMutableDictionary_CustomCallbacks_CFTypes() {
  // Reset counters
  retainCount = 0;
  releaseCount = 0;

  CFMutableDictionaryRef dict = CFDictionaryCreateMutable(
      NULL, 0, &testDefaultKeyCallBacks, &testDefaultValueCallBacks);
  if (dict == NULL) {
    return -1;
  }

  CFStringRef key =
      CFStringCreateWithCString(NULL, "Key", kCFStringEncodingASCII);
  CFStringRef value =
      CFStringCreateWithCString(NULL, "Value", kCFStringEncodingASCII);
  if (key == NULL || value == NULL) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(dict);
    return -2;
  }

  // Create copies to be stored in the dictionary
  CFStringRef key1 =
      CFStringCreateWithCString(NULL, "Key", kCFStringEncodingASCII);
  CFStringRef value1 =
      CFStringCreateWithCString(NULL, "Value", kCFStringEncodingASCII);

  int retainCountBefore = retainCount;
  int releaseCountBefore = releaseCount;

  CFDictionaryAddValue(dict, key1, value1);

  int deltaRetain = retainCount - retainCountBefore;
  int deltaRelease = releaseCount - releaseCountBefore;
  // For the purpose of this test, we only care about delta between
  // retain and release counts, e.g. receiving 1 retain and 1 release
  // has the same net effect as receiving 2 retains and 2 releases,
  // as delta for both of them is 0
  int globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != 2) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(key1);
    CFRelease(value1);
    CFRelease(dict);
    return -3;
  }

  // Release key1 and value1 since the dictionary has retained them
  CFRelease(key1);
  CFRelease(value1);

  const void *retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue == NULL) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(dict);
    return -4;
  }
  if (!CFEqual((CFStringRef)retrievedValue, value)) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(dict);
    return -5;
  }

  CFStringRef valueNew =
      CFStringCreateWithCString(NULL, "NewValue", kCFStringEncodingASCII);
  if (valueNew == NULL) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(dict);
    return -6;
  }

  retainCountBefore = retainCount;
  releaseCountBefore = releaseCount;

  CFDictionaryAddValue(dict, key, valueNew);

  // Since the key already exists, the new value should not be added
  deltaRetain = retainCount - retainCountBefore;
  deltaRelease = releaseCount - releaseCountBefore;
  globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != 0) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -7;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (!CFEqual((CFStringRef)retrievedValue, value)) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -8;
  }

  retainCountBefore = retainCount;
  releaseCountBefore = releaseCount;

  CFDictionarySetValue(dict, key, valueNew);

  deltaRetain = retainCount - retainCountBefore;
  deltaRelease = releaseCount - releaseCountBefore;
  globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != 0) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -9;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (!CFEqual((CFStringRef)retrievedValue, valueNew)) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -10;
  }

  retainCountBefore = retainCount;
  releaseCountBefore = releaseCount;

  CFDictionaryRemoveValue(dict, key);

  deltaRetain = retainCount - retainCountBefore;
  deltaRelease = releaseCount - releaseCountBefore;
  // The dictionary should release the key and value
  // So delta should be -2
  globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != -2) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -11;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (retrievedValue != NULL) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -12;
  }

  retainCountBefore = retainCount;
  releaseCountBefore = releaseCount;

  CFDictionaryAddValue(dict, key, value);

  deltaRetain = retainCount - retainCountBefore;
  deltaRelease = releaseCount - releaseCountBefore;
  // The dictionary should retain the key and value
  // So delta should be +2
  globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != 2) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -13;
  }

  retrievedValue = CFDictionaryGetValue(dict, key);
  if (!CFEqual((CFStringRef)retrievedValue, value)) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -14;
  }

  CFIndex count = CFDictionaryGetCount(dict);
  if (count != 1) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -15;
  }

  const void **keys = malloc(sizeof(void *) * count);
  const void **values = malloc(sizeof(void *) * count);
  if (keys == NULL || values == NULL) {
    free(keys);
    free(values);
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -16;
  }
  CFDictionaryGetKeysAndValues(dict, keys, values);

  if (!CFEqual((CFStringRef)keys[0], key) ||
      !CFEqual((CFStringRef)values[0], value)) {
    free(keys);
    free(values);
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -17;
  }
  free(keys);
  free(values);

  retainCountBefore = retainCount;
  releaseCountBefore = releaseCount;

  CFDictionaryRemoveAllValues(dict);

  deltaRetain = retainCount - retainCountBefore;
  deltaRelease = releaseCount - releaseCountBefore;
  // The dictionary should release the key and value
  // So delta should be -2
  globalDelta = deltaRetain - deltaRelease;

  if (globalDelta != -2) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -18;
  }

  count = CFDictionaryGetCount(dict);
  if (count != 0) {
    CFRelease(key);
    CFRelease(value);
    CFRelease(valueNew);
    CFRelease(dict);
    return -19;
  }

  CFRelease(key);
  CFRelease(value);
  CFRelease(valueNew);
  CFRelease(dict);

  return 0;
}

int test_lrint() {
  struct {
    double input;
    long int expected;
  } test_cases[] = {
      {0.0, 0L},
      {0.5, 0L},
      {1.0, 1L},
      {1.5, 2L},
      {2.0, 2L},
      {2.5, 2L},
      {3.0, 3L},
      {3.5, 4L},
      {4.5, 4L},
      {5.5, 6L},
      {-0.0, 0L},
      {-0.5, 0L},
      {-1.0, -1L},
      {-1.5, -2L},
      {-2.0, -2L},
      {-2.5, -2L},
      {-3.0, -3L},
      {-3.5, -4L},
      {-4.5, -4L},
      {-5.5, -6L},
      {1.4999999999, 1L},
      {1.5000000001, 2L},
      {-1.4999999999, -1L},
      {-1.5000000001, -2L},
      // Around INT_MAX
      {2147483647.0, 2147483647L},
      {2147483646.5, 2147483646L},
      {2147483647.4, 2147483647L},
      // Around INT_MIN
      {-2147483648.0, -2147483648L},
      {-2147483648.5, -2147483648L},
      {-2147483647.5, -2147483648L},
  };
  int num_tests = sizeof(test_cases) / sizeof(test_cases[0]);
  for (int i = 0; i < num_tests; i++) {
    double input = test_cases[i].input;
    long int expected = test_cases[i].expected;
    long int result = lrint(input);
    if (result != expected) {
      return -(i + 1);
    }
  }

  struct {
    float input;
    long int expected;
  } test_cases_f[] = {
      {0.0f, 0L},
      {0.5f, 0L},
      {1.0f, 1L},
      {1.5f, 2L},
      {2.0f, 2L},
      {2.5f, 2L},
      {3.0f, 3L},
      {3.5f, 4L},
      {4.5f, 4L},
      {5.5f, 6L},
      {-0.0f, 0L},
      {-0.5f, 0L},
      {-1.0f, -1L},
      {-1.5f, -2L},
      {-2.0f, -2L},
      {-2.5f, -2L},
      {-3.0f, -3L},
      {-3.5f, -4L},
      {-4.5f, -4L},
      {-5.5f, -6L},
      {1.4999999f, 1L},
      {1.5000001f, 2L},
      {-1.4999999f, -1L},
      {-1.5000001f, -2L},
#ifdef DEFINE_ME_WHEN_BUILDING_ON_MACOS
      // on macOS `long int` is 8 bytes
      {2147483648.0f, 2147483648L},
#else
      {2147483648.0f, 2147483647L}
#endif
  };
  int num_tests_f = sizeof(test_cases_f) / sizeof(test_cases_f[0]);
  for (int i = 0; i < num_tests_f; i++) {
    float input = test_cases_f[i].input;
    long int expected = test_cases_f[i].expected;
    long int result = lrintf(input);
    if (result != expected) {
      return -(num_tests + i + 1);
    }
  }

  return 0;
}

int test_fesetround() {
  int default_rounding = fegetround();
  if (default_rounding != FE_TONEAREST) {
    return -1;
  }
  if (lrint(+11.5) != +12.0 || lrint(+12.5) != +12.0 || lrint(-11.5) != -12.0) {
    return -2;
  }
  int res = fesetround(FE_TOWARDZERO);
  if (res != 0) {
    return -3;
  }
  if (lrint(+11.5) != +11.0 || lrint(+12.5) != +12.0 || lrint(-11.5) != -11.0) {
    return -4;
  }
  res = fesetround(default_rounding);
  if (res != 0) {
    return -5;
  }
  return 0;
}

int test_ldexp() {
  struct {
    double x;
    int n;
    double expected;
  } test_cases[] = {
      {0.0, 5, 0.0},  {-0.0, -3, -0.0}, {1.0, 0, 1.0},   {1.0, 1, 2.0},
      {1.0, -1, 0.5}, {2.5, 3, 20.0},   {3.0, -2, 0.75},
  };
  int num_tests = sizeof(test_cases) / sizeof(test_cases[0]);
  for (int i = 0; i < num_tests; i++) {
    double x = test_cases[i].x;
    int n = test_cases[i].n;
    double expected = test_cases[i].expected;
    double result = ldexp(x, n);

    if (expected != result) {
      return -(i + 1);
    }
  }

  struct {
    float x;
    int n;
    float expected;
  } test_cases_f[] = {
      {0.0f, 5, 0.0f},  {-0.0f, -3, -0.0f}, {1.0f, 0, 1.0f},   {1.0f, 1, 2.0f},
      {1.0f, -1, 0.5f}, {2.5f, 3, 20.0f},   {3.0f, -2, 0.75f},
  };
  int num_tests_f = sizeof(test_cases_f) / sizeof(test_cases_f[0]);
  for (int i = 0; i < num_tests_f; i++) {
    float x = test_cases_f[i].x;
    int n = test_cases_f[i].n;
    float expected = test_cases_f[i].expected;
    float result = ldexpf(x, n);

    if (expected != result) {
      return -(num_tests + i + 1);
    }
  }

  return 0;
}

// Just for readability, similar to _CTYPE_* constants
#define MASK_RUNE_ALPHA 0x00100L
#define MASK_RUNE_CONTROL 0x00200L
#define MASK_RUNE_DIGIT 0x00400L
#define MASK_RUNE_GRAPH 0x00800L
#define MASK_RUNE_LOWER 0x01000L
#define MASK_RUNE_PUNCT 0x02000L
#define MASK_RUNE_SPACE 0x04000L
#define MASK_RUNE_UPPER 0x08000L
#define MASK_RUNE_XDIGIT 0x10000L
#define MASK_RUNE_BLANK 0x20000L
#define MASK_RUNE_PRINT 0x40000L

int test_maskrune() {
  struct {
    char c;
    unsigned long mask;
    int expected;
  } test_cases[] = {
      {'A', MASK_RUNE_ALPHA, 256},    {'A', MASK_RUNE_UPPER, 32768},
      {'A', MASK_RUNE_GRAPH, 2048},   {'A', MASK_RUNE_LOWER, 0},

      {'z', MASK_RUNE_ALPHA, 256},    {'z', MASK_RUNE_LOWER, 4096},
      {'z', MASK_RUNE_GRAPH, 2048},   {'z', MASK_RUNE_UPPER, 0},

      {'5', MASK_RUNE_DIGIT, 1024},   {'5', MASK_RUNE_XDIGIT, 65536},
      {'5', MASK_RUNE_ALPHA, 0},

      {'?', MASK_RUNE_PUNCT, 8192},   {'?', MASK_RUNE_GRAPH, 2048},
      {'?', MASK_RUNE_PRINT, 262144}, {'?', MASK_RUNE_ALPHA, 0},

      {' ', MASK_RUNE_SPACE, 16384},  {' ', MASK_RUNE_BLANK, 131072},
      {' ', MASK_RUNE_PRINT, 262144}, {' ', MASK_RUNE_GRAPH, 0},

      {'\n', MASK_RUNE_CONTROL, 512}, {'\n', MASK_RUNE_PRINT, 0},
      {'\n', MASK_RUNE_GRAPH, 0},

      {'F', MASK_RUNE_XDIGIT, 65536}, {'G', MASK_RUNE_XDIGIT, 0},
  };

  int num_tests = sizeof(test_cases) / sizeof(test_cases[0]);
  for (int i = 0; i < num_tests; i++) {
    char c = test_cases[i].c;
    unsigned long mask = test_cases[i].mask;
    int expected = test_cases[i].expected;
    int result = __maskrune(c, mask);

    if (expected != result) {
      return -(i + 1);
    }
  }
  return 0;
}

int test_frexpf(void) {
  int exp_val;
  float m;

  /* Test 1: 8.0f = 0.5 * 2^4 */
  m = frexpf(8.0f, &exp_val);
  if (m != 0.5f || exp_val != 4)
    return -1;

  /* Test 2: 4.0f = 0.5 * 2^3 */
  m = frexpf(4.0f, &exp_val);
  if (m != 0.5f || exp_val != 3)
    return -2;

  /* Test 3: 0.75f is already normalized: 0.75f * 2^0 = 0.75f */
  m = frexpf(0.75f, &exp_val);
  if (m != 0.75f || exp_val != 0)
    return -3;

  /* Test 4: 1.0f = 0.5 * 2^1 */
  m = frexpf(1.0f, &exp_val);
  if (m != 0.5f || exp_val != 1)
    return -4;

  /* Test 5: 0.125f = 0.5 * 2^-2 */
  m = frexpf(0.125f, &exp_val);
  if (m != 0.5f || exp_val != -2)
    return -5;

  /* Test 6: 0.0f should return 0.0f and exponent 0 */
  m = frexpf(0.0f, &exp_val);
  if (m != 0.0f || exp_val != 0)
    return -6;

  /* Test 7: Negative value, -8.0f = -0.5 * 2^4 */
  m = frexpf(-8.0f, &exp_val);
  if (m != -0.5f || exp_val != 4)
    return -7;

  /* Test 8: -0.0f should be preserved (check with signbit) */
  m = frexpf(-0.0f, &exp_val);
  if (m != 0.0f || exp_val != 0)
    return -8;

  return 0;
}

int test_frexp() {
  double value, frac;
  int exp;

  // Test 1: 0.0 -> should return 0.0 and exponent 0.
  value = 0.0;
  frac = frexp(value, &exp);
  if (frac != 0.0 || exp != 0) {
    return -1;
  }

  // Test 2: 8.0 -> 8.0 = 0.5 * 2^4, so fraction 0.5 and exponent 4.
  value = 8.0;
  frac = frexp(value, &exp);
  if (frac != 0.5 || exp != 4) {
    return -2;
  }

  // Test 3: 0.75 -> already normalized, should return 0.75 and exponent 0.
  value = 0.75;
  frac = frexp(value, &exp);
  if (frac != 0.75 || exp != 0) {
    return -3;
  }

  // Test 4: -4.0 -> -4.0 = -0.5 * 2^3, so fraction -0.5 and exponent 3.
  value = -4.0;
  frac = frexp(value, &exp);
  if (frac != -0.5 || exp != 3) {
    return -4;
  }

  // Test 5: 1.0 -> 1.0 = 0.5 * 2^1, so fraction 0.5 and exponent 1.
  value = 1.0;
  frac = frexp(value, &exp);
  if (frac != 0.5 || exp != 1) {
    return -5;
  }

  // Test 6: pi -> 3.141592653589793 = (pi/4) * 2^2, expect fraction
  // ~0.7853981633974483 and exponent 2.
  value = 3.141592653589793;
  frac = frexp(value, &exp);
  if (exp != 2 || fabs(frac - (3.141592653589793 / 4.0)) > 1e-15) {
    return -6;
  }

  return 0;
}

void jmpfunction(jmp_buf env_buf) { longjmp(env_buf, 432); }

int test_setjmp() {
  int val;
  jmp_buf env_buffer;

  /* save calling environment for longjmp */
  val = setjmp(env_buffer);

  if (val != 0) {
    return val == 432 ? 0 : -2;
  }

  jmpfunction(env_buffer);

  return -1;
}

int test_inet_addr() {
  unsigned int res = inet_addr("127.0.0.1");
  if (res != 16777343) {
    return -1;
  }
  return 0;
}

int test_inet_ntop() {
  struct in_addr addr;
  char buffer[16]; // INET_ADDRSTRLEN

  unsigned int res = inet_addr("127.0.0.1");
  if (res != 16777343) {
    return -1;
  }

  addr.s_addr = res;
  if (inet_ntop(2, &addr, buffer, sizeof(buffer)) == NULL) {
    return -2;
  }

  if (strcmp(buffer, "127.0.0.1") != 0) {
    return -3;
  }

  return 0;
}

int test_inet_pton() {
  const char *ip_str = "127.0.0.1";
  struct in_addr addr;

  int res = inet_pton(2, ip_str, &addr);
  if (res <= 0) {
    return -1;
  }
  if (addr.s_addr != 16777343) {
    return -2;
  }
  return 0;
}

int test_case_CFURL(const char *basePathCStr, const char *urlPathCStr,
                    const char *fileNameCStr,
                    const char *expectedAppendedCStr) {
  CFURLRef url = CFURLCreateFromFileSystemRepresentation(
      NULL, (uint8_t *)urlPathCStr, strlen(urlPathCStr),
      1 // isDirectory
  );
  if (url == NULL) {
    return -1;
  }

  CFStringRef fileName =
      CFStringCreateWithCString(NULL, fileNameCStr, kCFStringEncodingASCII);
  CFURLRef appendedURL =
      CFURLCreateCopyAppendingPathComponent(NULL, url, fileName,
                                            0 // isDirectory
      );
  CFRelease(fileName);
  if (appendedURL == NULL) {
    CFRelease(url);
    return -2;
  }

  CFStringRef gotPath =
      CFURLCopyFileSystemPath(appendedURL, 0); // kCFURLPOSIXPathStyle
  if (gotPath == NULL) {
    CFRelease(appendedURL);
    CFRelease(url);
    return -3;
  }

  CFStringRef expectedAppended = CFStringCreateWithCString(
      NULL, expectedAppendedCStr, kCFStringEncodingASCII);
  if (!CFEqual(gotPath, expectedAppended)) {
    CFRelease(expectedAppended);
    CFRelease(gotPath);
    CFRelease(appendedURL);
    CFRelease(url);
    return -4;
  }
  CFRelease(expectedAppended);
  CFRelease(gotPath);

  CFURLRef deletedURL =
      CFURLCreateCopyDeletingLastPathComponent(NULL, appendedURL);
  if (deletedURL == NULL) {
    CFRelease(appendedURL);
    CFRelease(url);
    return -5;
  }

  gotPath = CFURLCopyFileSystemPath(deletedURL, 0); // kCFURLPOSIXPathStyle
  if (gotPath == NULL) {
    CFRelease(deletedURL);
    CFRelease(appendedURL);
    CFRelease(url);
    return -6;
  }

  CFStringRef expectedBase =
      CFStringCreateWithCString(NULL, basePathCStr, kCFStringEncodingASCII);
  if (!CFEqual(gotPath, expectedBase)) {
    CFRelease(expectedBase);
    CFRelease(gotPath);
    CFRelease(deletedURL);
    CFRelease(appendedURL);
    CFRelease(url);
    return -7;
  }

  CFRelease(expectedBase);
  CFRelease(gotPath);
  CFRelease(deletedURL);
  CFRelease(appendedURL);
  CFRelease(url);

  return 0;
}

int test_CFURL() {
  // base path, url path, filename, expected path
  int res = test_case_CFURL("/a/b/c", "/a/b/c", "test.txt", "/a/b/c/test.txt");
  if (res != 0) {
    return res;
  }
  res = test_case_CFURL("/a/b/c", "/a/b/c/", "test.txt", "/a/b/c/test.txt");
  if (res != 0) {
    return res - 10;
  }
  res = test_case_CFURL("/a/b/c", "/a/b/c/", "test.txt", "/a/b/c/test.txt");
  if (res != 0) {
    return res - 20;
  }
  return 0;
}

int test_CFNumberCompare_simple() {
  float a = 3.333;
  CFNumberRef aa = CFNumberCreate(NULL, 5, &a); // kCFNumberFloat32Type
  double b = 3.333;
  CFNumberRef bb = CFNumberCreate(NULL, 6, &b); // kCFNumberFloat64Type
  CFComparisonResult res = CFNumberCompare(aa, bb, NULL);
  // `3.333` looses precision as float, thus 2 numbers are not equal
  if (res != kCFCompareLessThan) {
    return -1;
  }
  res = CFNumberCompare(bb, aa, NULL);
  if (res != kCFCompareGreaterThan) {
    return -2;
  }
  int c = -1;
  CFNumberRef cc = CFNumberCreate(NULL, 3, &c); // kCFNumberSInt32Type
  long long d = -1;
  CFNumberRef dd = CFNumberCreate(NULL, 4, &d); // kCFNumberSInt64Type
  res = CFNumberCompare(cc, dd, NULL);
  if (res != kCFCompareEqualTo) {
    return -3;
  }
  char e = 0;
  CFNumberRef ee = CFNumberCreate(NULL, 1, &e); // kCFNumberSInt8Type
  double f = 0.0;
  CFNumberRef ff = CFNumberCreate(NULL, 6, &f); // kCFNumberFloat64Type
  res = CFNumberCompare(ee, ff, NULL);
  if (res != kCFCompareEqualTo) {
    return -4;
  }
  return 0;
}

#ifndef kCFNumberSInt8Type
#define kCFNumberSInt8Type 1
#define kCFNumberSInt16Type 2
#define kCFNumberSInt32Type 3
#define kCFNumberSInt64Type 4
#define kCFNumberFloat32Type 5
#define kCFNumberFloat64Type 6
#endif

static int cmp(CFNumberRef a, CFNumberRef b, CFComparisonResult expected,
               const char *label, int failCode) {
  CFComparisonResult r = CFNumberCompare(a, b, NULL);
  if (r != expected) {
    const char *expStr = expected == kCFCompareLessThan      ? "<"
                         : expected == kCFCompareGreaterThan ? ">"
                                                             : "==";
    const char *gotStr = r == kCFCompareLessThan      ? "<"
                         : r == kCFCompareGreaterThan ? ">"
                                                      : "==";
    printf("FAIL (%d): %s : expected %s, got %s\n", failCode, label, expStr,
           gotStr);
    return failCode;
  }
  return 0;
}

#define MAKE_NUM(var, typeEnum) CFNumberCreate(NULL, typeEnum, &(var))
#define TEST_CMP(aRef, bRef, expected, label, code)                            \
  {                                                                            \
    int _e = cmp(aRef, bRef, expected, label, code);                           \
    if (_e) {                                                                  \
      CFRelease(aRef);                                                         \
      CFRelease(bRef);                                                         \
      return _e;                                                               \
    }                                                                          \
    CFRelease(aRef);                                                           \
    CFRelease(bRef);                                                           \
  }

static int compare_integral_examples(void) {
  /* Cross-width equalities */
  {
    int32_t v32 = -1;
    int64_t v64 = -1;
    CFNumberRef n32 = MAKE_NUM(v32, kCFNumberSInt32Type);
    CFNumberRef n64 = MAKE_NUM(v64, kCFNumberSInt64Type);
    TEST_CMP(n32, n64, kCFCompareEqualTo, "SInt32 -1 == SInt64 -1", -10);
  }
  {
    int8_t z8 = 0;
    double zD = 0.0;
    CFNumberRef n8 = MAKE_NUM(z8, kCFNumberSInt8Type);
    CFNumberRef nD = MAKE_NUM(zD, kCFNumberFloat64Type);
    TEST_CMP(n8, nD, kCFCompareEqualTo, "SInt8 0 == Float64 0.0", -11);
  }

  /* Min / Max ordering across widths */
  {
    int64_t min64 = INT64_MIN;
    int32_t min32 = INT32_MIN;
    CFNumberRef n64 = MAKE_NUM(min64, kCFNumberSInt64Type);
    CFNumberRef n32 = MAKE_NUM(min32, kCFNumberSInt32Type);
    TEST_CMP(n64, n32, kCFCompareLessThan, "INT64_MIN < INT32_MIN", -12);
  }
  {
    int64_t max64 = INT64_MAX;
    int32_t max32 = INT32_MAX;
    CFNumberRef n64 = MAKE_NUM(max64, kCFNumberSInt64Type);
    CFNumberRef n32 = MAKE_NUM(max32, kCFNumberSInt32Type);
    TEST_CMP(n64, n32, kCFCompareGreaterThan, "INT64_MAX > INT32_MAX", -13);
  }
  {
    int16_t min16 = INT16_MIN; /* -32768 */
    int8_t min8 = INT8_MIN;    /* -128   */
    CFNumberRef n16 = MAKE_NUM(min16, kCFNumberSInt16Type);
    CFNumberRef n8 = MAKE_NUM(min8, kCFNumberSInt8Type);
    TEST_CMP(n16, n8, kCFCompareLessThan, "INT16_MIN < INT8_MIN", -14);
  }
  {
    int16_t max16 = INT16_MAX;
    int8_t max8 = INT8_MAX;
    CFNumberRef n16 = MAKE_NUM(max16, kCFNumberSInt16Type);
    CFNumberRef n8 = MAKE_NUM(max8, kCFNumberSInt8Type);
    TEST_CMP(n16, n8, kCFCompareGreaterThan, "INT16_MAX > INT8_MAX", -15);
  }

  /* Extremes vs -1 */
  {
    int64_t min64 = INT64_MIN;
    int64_t neg1 = -1;
    CFNumberRef nMin = MAKE_NUM(min64, kCFNumberSInt64Type);
    CFNumberRef nNeg1 = MAKE_NUM(neg1, kCFNumberSInt64Type);
    TEST_CMP(nMin, nNeg1, kCFCompareLessThan, "INT64_MIN < -1", -16);
  }

  return 0;
}

static int compare_precision_examples(void) {
  /* Original float vs double 3.333 */
  {
    float f = 3.333f;
    double d = 3.333;
    CFNumberRef nf = MAKE_NUM(f, kCFNumberFloat32Type);
    CFNumberRef nd = MAKE_NUM(d, kCFNumberFloat64Type);
    /* float loses precision => float < double (expected) */
    TEST_CMP(nf, nd, kCFCompareLessThan, "float 3.333f < double 3.333", -20);
    /* Reverse */
    float f2 = 3.333f;
    double d2 = 3.333;
    CFNumberRef nf2 = MAKE_NUM(f2, kCFNumberFloat32Type);
    CFNumberRef nd2 = MAKE_NUM(d2, kCFNumberFloat64Type);
    TEST_CMP(nd2, nf2, kCFCompareGreaterThan, "double 3.333 > float 3.333f",
             -21);
  }

  /* 0.1f vs 0.1 (0.1f rounds *up* relative to double literal 0.1) */
  {
    float f = 0.1f;
    double d = 0.1; /* double literal */
    CFNumberRef nf = MAKE_NUM(f, kCFNumberFloat32Type);
    CFNumberRef nd = MAKE_NUM(d, kCFNumberFloat64Type);
    /* 0.1f (promoted) is slightly greater than 0.1 double */
    TEST_CMP(nf, nd, kCFCompareGreaterThan, "0.1f > 0.1 (double)", -22);
  }

  /* INT64_MAX vs its double representation (double rounds) */
  {
    int64_t i = INT64_MAX;        /*  9223372036854775807 */
    double d = (double)INT64_MAX; /* Rounds to 9223372036854775808 */
    CFNumberRef ni = MAKE_NUM(i, kCFNumberSInt64Type);
    CFNumberRef nd = MAKE_NUM(d, kCFNumberFloat64Type);
    TEST_CMP(ni, nd, kCFCompareLessThan,
             "INT64_MAX (exact) < double(INT64_MAX) (rounded up)", -23);
  }

  return 0;
}

static int compare_special_float_values(void) {
  /* Positive vs negative zero */
  {
    double pz = 0.0;
    double nz = -0.0;
    CFNumberRef nP = MAKE_NUM(pz, kCFNumberFloat64Type);
    CFNumberRef nN = MAKE_NUM(nz, kCFNumberFloat64Type);
    TEST_CMP(nP, nN, kCFCompareEqualTo, "+0.0 == -0.0", -24);
  }

  /* Infinities */
  {
    double inf = INFINITY;
    double ninf = -INFINITY;
    double zero = 0.0;

    CFNumberRef nInf = MAKE_NUM(inf, kCFNumberFloat64Type);
    CFNumberRef nZero = MAKE_NUM(zero, kCFNumberFloat64Type);
    TEST_CMP(nInf, nZero, kCFCompareGreaterThan, "Inf  > 0", -25);

    nZero = MAKE_NUM(zero, kCFNumberFloat64Type);
    CFNumberRef nNInf = MAKE_NUM(ninf, kCFNumberFloat64Type);
    TEST_CMP(nNInf, nZero, kCFCompareLessThan, "-Inf < 0", -26);

    nInf = MAKE_NUM(inf, kCFNumberFloat64Type);
    nNInf = MAKE_NUM(ninf, kCFNumberFloat64Type);
    TEST_CMP(nInf, nNInf, kCFCompareGreaterThan, "Inf  > -Inf", -27);
  }

  return 0;
}

static int compare_unsigned_limit_examples(void) {
  /* UINT64_MAX cannot be stored exactly as a signed 64-bit CFNumber.
     We *demonstrate* by comparing a double approximation vs INT64_MAX. */
  {
    double u64d = (double)UINT64_MAX; /* ~1.844674407e19 (loses low bits) */
    int64_t i64max = INT64_MAX;       /*  9.223372036854775807e18 */
    CFNumberRef nUApprox = MAKE_NUM(u64d, kCFNumberFloat64Type);
    CFNumberRef nI64Max = MAKE_NUM(i64max, kCFNumberSInt64Type);
    TEST_CMP(nUApprox, nI64Max, kCFCompareGreaterThan,
             "double(UINT64_MAX) > INT64_MAX", -28);
  }

  /* Similar for smaller widths: compare UINT32_MAX via double vs INT32_MAX
   * (exact vs rounding) */
  {
    double u32d = (double)UINT32_MAX; /* 4294967295 exactly representable */
    int32_t s32max = INT32_MAX;       /* 2147483647 */
    CFNumberRef nU = MAKE_NUM(u32d, kCFNumberFloat64Type);
    CFNumberRef nS = MAKE_NUM(s32max, kCFNumberSInt32Type);
    TEST_CMP(nU, nS, kCFCompareGreaterThan, "double(UINT32_MAX) > INT32_MAX",
             -29);
  }

  /* UINT8_MAX vs INT8_MAX using a wider signed container (int16) for 255 */
  {
    int16_t u8max_as16 = 255; /* representable */
    int8_t s8max = INT8_MAX;  /* 127 */
    CFNumberRef nU = MAKE_NUM(u8max_as16, kCFNumberSInt16Type);
    CFNumberRef nS = MAKE_NUM(s8max, kCFNumberSInt8Type);
    TEST_CMP(nU, nS, kCFCompareGreaterThan, "255 (as SInt16) > INT8_MAX", -30);
  }

  return 0;
}

int test_CFNumberCompare_extended(void) {
  int r;

  r = compare_integral_examples();
  if (r)
    return r;
  r = compare_precision_examples();
  if (r)
    return r;
  r = compare_special_float_values();
  if (r)
    return r;
  r = compare_unsigned_limit_examples();
  if (r)
    return r;

  return 0;
}

int test_memset_pattern() {
  char buf[64];
  // memset_pattern4
  memset_pattern4(buf, "1234", sizeof(buf));
  if (strncmp(buf, "1234123412", 10) != 0) {
    return -1;
  }
  memset(buf, 0, sizeof(buf));
  memset_pattern4(buf, "abcd", 8);
  if (memcmp(buf, "abcdabcd", 8) != 0) {
    return -2;
  }
  memset(buf, 0, sizeof(buf));
  memset_pattern4(buf, "XYZW", 3);
  if (memcmp(buf, "XYZ", 3) != 0) {
    return -3;
  }
  char original_buf[sizeof(buf)];
  memset(buf, 0xAA, sizeof(buf)); // Fill buffer with a known value
  memcpy(original_buf, buf, sizeof(buf));
  memset_pattern4(buf, "1234", 0);
  if (memcmp(buf, original_buf, sizeof(buf)) != 0) {
    return -4;
  }
  memset(buf, 0, sizeof(buf));
  char pattern4_null[] = {'A', '\0', 'B', 'C'};
  char expected4_null[] = {'A', '\0', 'B', 'C', 'A', '\0', 'B'};
  memset_pattern4(buf, pattern4_null, 7);
  if (memcmp(buf, expected4_null, 7) != 0) {
    return -5;
  }
  // memset_pattern8
  unsigned long long pattern8 = 0x0102030405060708;
  char expected8_full[] = "\x08\x07\x06\x05\x04\x03\x02\x01";
  memset(buf, 0, sizeof(buf));
  memset_pattern8(buf, &pattern8, 10);
  if (memcmp(buf, expected8_full, 8) != 0 ||
      memcmp(buf + 8, expected8_full, 2) != 0) {
    return -6;
  }
  memset(buf, 0, sizeof(buf));
  memset_pattern8(buf, &pattern8, 16);
  if (memcmp(buf, expected8_full, 8) != 0 ||
      memcmp(buf + 8, expected8_full, 8) != 0) {
    return -7;
  }
  memset(buf, 0, sizeof(buf));
  memset_pattern8(buf, &pattern8, 5);
  if (memcmp(buf, expected8_full, 5) != 0) {
    return -8;
  }
  // memset_pattern16
  const char *pattern16 = "0123456789ABCDEF";
  memset(buf, 0, sizeof(buf));
  memset_pattern16(buf, pattern16, 20);
  char expected16_trunc[] = "0123456789ABCDEF0123";
  if (memcmp(buf, expected16_trunc, 20) != 0) {
    return -9;
  }
  memset(buf, 0, sizeof(buf));
  memset_pattern16(buf, pattern16, 32);
  char expected16_exact[] = "0123456789ABCDEF0123456789ABCDEF";
  if (memcmp(buf, expected16_exact, 32) != 0) {
    return -10;
  }
  return 0;
}
typedef struct {
  SyncTester *tester;
  BOOL res;
} sync_test_arg;

void *modify(sync_test_arg *arg) {
  SyncTester *tester = arg->tester;
  arg->res = [tester holdAndCheckCounter];
  return NULL;
}
void *try_modify(SyncTester *tester) {
  [tester tryModifyCounter];
  return NULL;
}

int test_synchronized() {
  SyncTester *sync_test = [SyncTester new];
  sync_test_arg *arg = malloc(sizeof(sync_test_arg));
  memset(arg, 0, sizeof(sync_test_arg));
  arg->tester = sync_test;
  pthread_t locking_thread;
  pthread_create(&locking_thread, NULL, (void *(*)(void *)) & modify, arg);
  pthread_t blocked_threads[10];
  for (int i = 0; i < 10; i++) {
    pthread_create(blocked_threads + i, NULL, (void *(*)(void *)) & try_modify,
                   sync_test);
  }
  if (pthread_join(locking_thread, NULL))
    return -1;
  if (!arg->res)
    return -1;
  [sync_test recursiveSyncEnter];
  if (!sync_test.test_ok)
    return -1;
  return 0;
}

bool test_case_CFURLHasDirectoryPath(const char *str) {
  CFURLRef url = CFURLCreateWithBytes(NULL, str, strlen(str),
                                      kCFStringEncodingASCII, NULL);

  if (!url) {
    return false;
  }

  Boolean res = CFURLHasDirectoryPath(url);
  CFRelease(url);
  return res;
}

int test_CFURLHasDirectoryPath() {
  if (test_case_CFURLHasDirectoryPath("/a/b"))
    return -1;
  if (!test_case_CFURLHasDirectoryPath("/a/b/"))
    return -2;
  if (!test_case_CFURLHasDirectoryPath("/"))
    return -3;
  if (test_case_CFURLHasDirectoryPath("//"))
    return -4;
  if (test_case_CFURLHasDirectoryPath("//a"))
    return -5;
  if (!test_case_CFURLHasDirectoryPath("//a/"))
    return -6;
  if (!test_case_CFURLHasDirectoryPath("///"))
    return -7;
  if (!test_case_CFURLHasDirectoryPath("////"))
    return -8;
  if (!test_case_CFURLHasDirectoryPath("."))
    return -9;
  if (!test_case_CFURLHasDirectoryPath(".."))
    return -10;
  if (test_case_CFURLHasDirectoryPath("..."))
    return -11;
  if (!test_case_CFURLHasDirectoryPath("/.."))
    return -12;
  if (test_case_CFURLHasDirectoryPath(""))
    return -13;
  return 0;
}

int test_NSMutableString_deleteCharactersInRange() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  NSMutableString *str = [NSMutableString stringWithUTF8String:"abc"];
  NSRange r1 = {0, 3};
  [str deleteCharactersInRange:r1];
  NSString *expected = [NSString stringWithUTF8String:""];
  if (!CFEqual(str, expected)) {
    return -1;
  }
  str = [NSMutableString stringWithUTF8String:"abc"];
  NSRange r2 = {1, 1};
  [str deleteCharactersInRange:r2];
  expected = [NSString stringWithUTF8String:"ac"];
  if (!CFEqual(str, expected)) {
    return -2;
  }
  str = [NSMutableString stringWithUTF8String:"abc"];
  NSRange r3 = {0, 2};
  [str deleteCharactersInRange:r3];
  expected = [NSString stringWithUTF8String:"c"];
  if (!CFEqual(str, expected)) {
    return -3;
  }
  [pool drain];
  return 0;
}

int test_NSString_stringByReplacingOccurrencesOfString() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Simple replacement
  NSString *str = [NSString stringWithUTF8String:"hello world"];
  NSString *target = [NSString stringWithUTF8String:"world"];
  NSString *replacement = [NSString stringWithUTF8String:"touchHLE"];
  NSString *res = [str stringByReplacingOccurrencesOfString:target
                                                 withString:replacement];
  NSString *expected = [NSString stringWithUTF8String:"hello touchHLE"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -1;
  }

  // Multiple occurrences
  str = [NSString stringWithUTF8String:"aaa"];
  target = [NSString stringWithUTF8String:"a"];
  replacement = [NSString stringWithUTF8String:"b"];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"bbb"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -2;
  }

  // Overlapping occurrences (should not be replaced multiple times)
  str = [NSString stringWithUTF8String:"aaaa"];
  target = [NSString stringWithUTF8String:"aa"];
  replacement = [NSString stringWithUTF8String:"b"];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"bb"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -3;
  }

  // No occurrences
  str = [NSString stringWithUTF8String:"hello"];
  target = [NSString stringWithUTF8String:"world"];
  replacement = [NSString stringWithUTF8String:"!"];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"hello"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -4;
  }

  // Replace with empty string
  str = [NSString stringWithUTF8String:"hello world"];
  target = [NSString stringWithUTF8String:"world"];
  replacement = [NSString stringWithUTF8String:""];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"hello "];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -5;
  }

  // Replace whole string
  str = [NSString stringWithUTF8String:"hello"];
  target = [NSString stringWithUTF8String:"hello"];
  replacement = [NSString stringWithUTF8String:"world"];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"world"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -6;
  }

  // Empty target (macOS behavior: returns original string)
  str = [NSString stringWithUTF8String:"hello"];
  target = [NSString stringWithUTF8String:""];
  replacement = [NSString stringWithUTF8String:"!"];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:"hello"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -7;
  }

  // Source, target and replacement empty
  str = [NSString stringWithUTF8String:""];
  target = [NSString stringWithUTF8String:""];
  replacement = [NSString stringWithUTF8String:""];
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement];
  expected = [NSString stringWithUTF8String:""];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -8;
  }

  [pool drain];
  return 0;
}

int test_NSString_stringByReplacingOccurrencesOfString_options_range() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Case insensitive replacement
  NSString *str = [NSString stringWithUTF8String:"Hello HELLO hello"];
  NSString *target = [NSString stringWithUTF8String:"hello"];
  NSString *replacement = [NSString stringWithUTF8String:"hi"];
  NSRange range = NSMakeRange(0, 17);
  NSString *res =
      [str stringByReplacingOccurrencesOfString:target
                                     withString:replacement
                                        options:NSCaseInsensitiveSearch
                                          range:range];
  NSString *expected = [NSString stringWithUTF8String:"hi hi hi"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -1;
  }

  // Replacement within a range
  str = [NSString stringWithUTF8String:"[hello] hello [hello]"];
  target = [NSString stringWithUTF8String:"hello"];
  replacement = [NSString stringWithUTF8String:"hi"];
  range = NSMakeRange(7, 7); // Only the middle "hello"
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement
                                          options:0
                                            range:range];
  expected = [NSString stringWithUTF8String:"[hello] hi [hello]"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -2;
  }

  // Case insensitive within a range
  str = [NSString stringWithUTF8String:"AAA aaa AAA"];
  target = [NSString stringWithUTF8String:"AAA"];
  replacement = [NSString stringWithUTF8String:"B"];
  range = NSMakeRange(0, 7); // "AAA aaa"
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement
                                          options:NSCaseInsensitiveSearch
                                            range:range];
  expected = [NSString stringWithUTF8String:"B B AAA"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -3;
  }

  // Range at the end
  str = [NSString stringWithUTF8String:"hello hello"];
  target = [NSString stringWithUTF8String:"hello"];
  replacement = [NSString stringWithUTF8String:"world"];
  range = NSMakeRange(6, 5);
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement
                                          options:0
                                            range:range];
  expected = [NSString stringWithUTF8String:"hello world"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -4;
  }

  // Empty range
  str = [NSString stringWithUTF8String:"hello"];
  target = [NSString stringWithUTF8String:"hello"];
  replacement = [NSString stringWithUTF8String:"world"];
  range = NSMakeRange(2, 0);
  res = [str stringByReplacingOccurrencesOfString:target
                                       withString:replacement
                                          options:0
                                            range:range];
  expected = [NSString stringWithUTF8String:"hello"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -5;
  }

  [pool drain];
  return 0;
}

int test_NSString_pathWithComponents() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Absolute path
  NSArray *components =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"/"],
                                [NSString stringWithUTF8String:"a"],
                                [NSString stringWithUTF8String:"b"], nil];
  NSString *res = [NSString pathWithComponents:components];
  NSString *expected = [NSString stringWithUTF8String:"/a/b"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -1;
  }

  // Relative path
  components =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"a"],
                                [NSString stringWithUTF8String:"b"], nil];
  res = [NSString pathWithComponents:components];
  expected = [NSString stringWithUTF8String:"a/b"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -2;
  }

  // No redundant slashes
  components =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"a/"],
                                [NSString stringWithUTF8String:"/b"], nil];
  res = [NSString pathWithComponents:components];
  expected = [NSString stringWithUTF8String:"a/b"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -3;
  }

  // Single component
  components =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"a"], nil];
  res = [NSString pathWithComponents:components];
  expected = [NSString stringWithUTF8String:"a"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -4;
  }

  // Empty array
  components = [NSArray array];
  res = [NSString pathWithComponents:components];
  expected = [NSString stringWithUTF8String:""];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -5;
  }

  // Empty strings inside components
  components =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"a"],
                                [NSString stringWithUTF8String:""],
                                [NSString stringWithUTF8String:""],
                                [NSString stringWithUTF8String:"b"], nil];
  res = [NSString pathWithComponents:components];
  expected = [NSString stringWithUTF8String:"a/b"];
  if (![res isEqualToString:expected]) {
    [pool drain];
    return -6;
  }

  [pool drain];
  return 0;
}

int test_strptime() {
  struct tm tm;
  memset(&tm, 0, sizeof(struct tm));
  char *res = strptime("12:34:56,", "%H:%M:%S,", &tm);
  if (res == NULL || *res != '\0') {
    return -1;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -2;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("01:02:03,", "%H:%M:%S,", &tm);
  if (res == NULL || *res != '\0') {
    return -3;
  }
  if (tm.tm_hour != 1 || tm.tm_min != 2 || tm.tm_sec != 3) {
    return -4;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("invalid", "%H:%M:%S,", &tm);
  if (res != NULL) {
    return -5;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12:34:56,extra", "%H:%M:%S,", &tm);
  if (res == NULL || strcmp(res, "extra") != 0) {
    return -6;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -7;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12   :34: 56", "%H : %M : %S", &tm);
  if (res == NULL || *res != '\0') {
    return -8;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -9;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12:34:56", "%H :%M :%S", &tm);
  if (res == NULL || *res != '\0') {
    return -10;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -11;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12\t\n :34\f:56", "%H :%M :%S", &tm);
  if (res == NULL || *res != '\0') {
    return -12;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -13;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("  12:34:56  ", " %H:%M:%S ", &tm);
  if (res == NULL || *res != '\0') {
    return -14;
  }
  if (tm.tm_hour != 12 || tm.tm_min != 34 || tm.tm_sec != 56) {
    return -15;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("XX:34:56", "%H:%M:%S", &tm);
  if (res != NULL) {
    return -16;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12:XX:56", "%H:%M:%S", &tm);
  if (res != NULL) {
    return -17;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("12:34:XX", "%H:%M:%S", &tm);
  if (res != NULL) {
    return -18;
  }

  memset(&tm, 0, sizeof(struct tm));
  res = strptime("10\r\n", "%H:%M:%S,", &tm);
  if (res != NULL) {
    return -19;
  }

  return 0;
}

int test_strftime() {
  struct tm tm;
  char buf[64];
  memset(&tm, 0, sizeof(struct tm));
  tm.tm_mon = 0; // January
  tm.tm_mday = 31;
  tm.tm_hour = 12;
  tm.tm_min = 34;

  size_t res = strftime(buf, sizeof(buf), "%m/%d     %H:%M", &tm);
  if (res == 0) {
    return -1;
  }
  if (strcmp(buf, "01/31     12:34") != 0) {
    return -2;
  }

  memset(&tm, 0, sizeof(struct tm));
  tm.tm_mon = 10; // November
  tm.tm_mday = 5;
  tm.tm_hour = 9;
  tm.tm_min = 7;

  res = strftime(buf, sizeof(buf), "%m/%d     %H:%M", &tm);
  if (res == 0) {
    return -3;
  }
  if (strcmp(buf, "11/05     09:07") != 0) {
    return -4;
  }

  // macOS treats the unsupported '#' conversion literally, dropping the '%'.
  res = strftime(buf, sizeof(buf), "%#c", &tm);
  if (res != 2) {
    return -5;
  }
  if (strcmp(buf, "#c") != 0) {
    return -6;
  }

  memset(&tm, 0, sizeof(struct tm));
  tm.tm_year = 2026 - 1900;

  // The formatted year and its terminator fit exactly.
  res = strftime(buf, 5, "%Y", &tm);
  if (res != 4) {
    return -7;
  }
  if (strcmp(buf, "2026") != 0) {
    return -8;
  }

  // Insufficient space must return zero, including when only the NUL won't fit.
  res = strftime(buf, 4, "%Y", &tm);
  if (res != 0) {
    return -9;
  }
  res = strftime(buf, 1, "%Y", &tm);
  if (res != 0) {
    return -10;
  }

  return 0;
}

int test_wcsftime() {
  struct tm tm;
  wchar_t buf[5];
  memset(&tm, 0, sizeof(struct tm));
  tm.tm_year = 2026 - 1900;

  // The formatted year and its terminator fit exactly.
  size_t res = wcsftime(buf, sizeof(buf) / sizeof(buf[0]), L"%Y", &tm);
  if (res != 4) {
    return -1;
  }
  if (wcscmp(buf, L"2026") != 0) {
    return -2;
  }

  // Insufficient space must return zero, including when only the NUL won't fit.
  res = wcsftime(buf, 4, L"%Y", &tm);
  if (res != 0) {
    return -3;
  }
  res = wcsftime(buf, 1, L"%Y", &tm);
  if (res != 0) {
    return -4;
  }

  return 0;
}

@interface InvocationTarget : NSObject {
@public
  id receivedValue;
  const char *cstringValue;
  int intValue;
}
- (void)storeValue:(id)value;
- (void)clearValue;
- (void)storeCString:(const char *)str;
- (void)storeIntPtr:(int *)ptr;
@end

@implementation InvocationTarget
- (void)storeValue:(id)value {
  receivedValue = value;
}
- (void)clearValue {
  receivedValue = nil;
}
- (void)storeCString:(const char *)str {
  cstringValue = str;
}
- (void)storeIntPtr:(int *)ptr {
  intValue = *ptr;
}
@end

static BOOL g_deallocTrackerDidDealloc = NO;

@interface DeallocTracker : NSObject
@end

@implementation DeallocTracker
- (void)dealloc {
  g_deallocTrackerDidDealloc = YES;
  [super dealloc];
}
@end

int test_NSMethodSignature() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // "v12@0:4@8" = void return, 3 args: self(@), _cmd(:), one id(@)
  NSMethodSignature *sig =
      [NSMethodSignature signatureWithObjCTypes:"v12@0:4@8"];

  if ([sig numberOfArguments] != 3) {
    [pool drain];
    return -1;
  }

  // methodReturnType should be "v"
  if (strcmp([sig methodReturnType], "v") != 0) {
    [pool drain];
    return -2;
  }

  // arg 0 = "@" (self)
  if (strcmp([sig getArgumentTypeAtIndex:0], "@") != 0) {
    [pool drain];
    return -3;
  }

  // arg 1 = ":" (SEL)
  if (strcmp([sig getArgumentTypeAtIndex:1], ":") != 0) {
    [pool drain];
    return -4;
  }

  // arg 2 = "@" (id argument)
  if (strcmp([sig getArgumentTypeAtIndex:2], "@") != 0) {
    [pool drain];
    return -5;
  }

  // "v8@0:4" = void return, 2 args: self, _cmd (no extra args)
  NSMethodSignature *sig2 = [NSMethodSignature signatureWithObjCTypes:"v8@0:4"];
  if ([sig2 numberOfArguments] != 2) {
    [pool drain];
    return -6;
  }
  if (strcmp([sig2 methodReturnType], "v") != 0) {
    [pool drain];
    return -7;
  }

  // "v12@0:4^i8" = void return, 3 args: self(@), _cmd(:), pointer-to-int(^i)
  NSMethodSignature *sig3 =
      [NSMethodSignature signatureWithObjCTypes:"v12@0:4^i8"];
  if ([sig3 numberOfArguments] != 3) {
    [pool drain];
    return -8;
  }
  if (strcmp([sig3 methodReturnType], "v") != 0) {
    [pool drain];
    return -9;
  }
  if (strcmp([sig3 getArgumentTypeAtIndex:2], "^i") != 0) {
    [pool drain];
    return -10;
  }

  [pool drain];
  return 0;
}

int test_NSInvocation() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  InvocationTarget *target = [InvocationTarget new];
  NSMethodSignature *sig =
      [NSMethodSignature signatureWithObjCTypes:"v12@0:4@8"];
  NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];

  [inv setTarget:target];
  SEL sel = NSSelectorFromString([NSString stringWithUTF8String:"storeValue:"]);
  [inv setSelector:sel];

  // setArgument:atIndex: takes a pointer to the argument value
  NSObject *val = [NSObject new];
  [inv setArgument:&val atIndex:2];
  [inv invoke];

  if (target->receivedValue != val) {
    [pool drain];
    return -1;
  }

  [pool drain];
  return 0;
}

int test_NSInvocation_invokeWithTarget() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  InvocationTarget *target1 = [InvocationTarget new];
  InvocationTarget *target2 = [InvocationTarget new];
  NSMethodSignature *sig =
      [NSMethodSignature signatureWithObjCTypes:"v12@0:4@8"];
  NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];

  [inv setTarget:target1];
  SEL sel = NSSelectorFromString([NSString stringWithUTF8String:"storeValue:"]);
  [inv setSelector:sel];

  NSObject *val = [NSObject new];
  [inv setArgument:&val atIndex:2];

  // invokeWithTarget: should use target2, not target1
  [inv invokeWithTarget:target2];

  if (target2->receivedValue != val) {
    [pool drain];
    return -1;
  }

  [pool drain];
  return 0;
}

int test_NSInvocation_retainArguments() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Test 1: object argument is retained by the invocation.
  {
    InvocationTarget *target = [InvocationTarget new];
    NSMethodSignature *sig =
        [NSMethodSignature signatureWithObjCTypes:"v12@0:4@8"];
    NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];
    [inv setTarget:target];
    SEL sel =
        NSSelectorFromString([NSString stringWithUTF8String:"storeValue:"]);
    [inv setSelector:sel];

    NSObject *val = [[NSObject alloc] init]; // retainCount = 1
    NSUInteger before = [val retainCount];
    [inv setArgument:&val atIndex:2];
    [inv retainArguments]; // invocation must retain val
    if ([val retainCount] != before + 1) {
      [pool drain];
      return -1;
    }
    // Invoke still works; val is alive because the invocation holds it.
    [inv invoke];
    if (target->receivedValue != val) {
      [pool drain];
      return -2;
    }
    [val release]; // balance our alloc; invocation still holds one ref
  }

  // Test 2: C string argument is copied so the invocation owns the bytes.
  {
    InvocationTarget *target = [InvocationTarget new];
    // "v12@0:4*8" = void, self(@), _cmd(:), const char *(*)
    NSMethodSignature *sig =
        [NSMethodSignature signatureWithObjCTypes:"v12@0:4*8"];
    NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];
    [inv setTarget:target];
    SEL sel =
        NSSelectorFromString([NSString stringWithUTF8String:"storeCString:"]);
    [inv setSelector:sel];

    char buf[16];
    strcpy(buf, "hello");
    const char *ptr = buf;
    [inv setArgument:&ptr atIndex:2];
    [inv retainArguments]; // must copy "hello" into invocation-owned memory

    // Overwrite original buffer; invocation must pass its copy, not buf.
    strcpy(buf, "world");

    [inv invoke];
    if (strcmp(target->cstringValue, "hello") != 0) {
      [pool drain];
      return -3;
    }
  }

  // Test 3: invocation keeps @ arg alive after caller drops its reference.
  {
    InvocationTarget *target = [InvocationTarget new];
    NSMethodSignature *sig =
        [NSMethodSignature signatureWithObjCTypes:"v12@0:4@8"];
    NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];
    [inv setTarget:target];
    SEL sel =
        NSSelectorFromString([NSString stringWithUTF8String:"storeValue:"]);
    [inv setSelector:sel];

    g_deallocTrackerDidDealloc = NO;
    DeallocTracker *val = [[DeallocTracker alloc] init];
    DeallocTracker *weakVal = val; // un-retained alias
    [inv setArgument:&val atIndex:2];
    [inv retainArguments]; // invocation owns its own reference now
    [val release];         // caller drops its reference
    val = nil;

    // Without the invocation's retain, val would now be deallocated.
    if (g_deallocTrackerDidDealloc) {
      [pool drain];
      return -4;
    }
    [inv invoke];
    if (target->receivedValue != weakVal) {
      [pool drain];
      return -5;
    }
    if (g_deallocTrackerDidDealloc) {
      [pool drain];
      return -6;
    }
  }

  [pool drain];
  return 0;
}

int test_NSInvocation_pointer() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  InvocationTarget *target = [InvocationTarget new];
  // "v12@0:4^i8" = void, self(@), _cmd(:), int *(^i)
  NSMethodSignature *sig =
      [NSMethodSignature signatureWithObjCTypes:"v12@0:4^i8"];
  NSInvocation *inv = [NSInvocation invocationWithMethodSignature:sig];

  [inv setTarget:target];
  SEL sel =
      NSSelectorFromString([NSString stringWithUTF8String:"storeIntPtr:"]);
  [inv setSelector:sel];

  int x = 42;
  int *ptr = &x;
  [inv setArgument:&ptr atIndex:2];
  [inv invoke];

  if (target->intValue != 42) {
    [pool drain];
    return -1;
  }

  [pool drain];
  return 0;
}

@interface CharBufferObject : NSObject {
@public
  char *buffer;
  NSUInteger length;

  char *badKeyBuffer;
  NSUInteger badKeyLength;
}
@end

@implementation CharBufferObject
- (instancetype)initWithBytes:(const char *)b length:(NSUInteger)l {
  self = [super init];
  length = l;
  buffer = b;
  badKeyLength = -1;
  badKeyBuffer = b;
  return self;
}

- (void)dealloc {
  free(buffer);
}

- (void)encodeWithCoder:(NSCoder *)coder {
  [coder encodeBytes:buffer
              length:length
              forKey:[NSString stringWithUTF8String:"buffer"]];
}

- (instancetype)initWithCoder:(NSCoder *)coder {
  self = [super init];
  char *temp_buffer =
      [coder decodeBytesForKey:[NSString stringWithUTF8String:"buffer"]
                returnedLength:&length];
  buffer = malloc(length);
  memcpy(buffer, temp_buffer, length);

  badKeyBuffer =
      [coder decodeBytesForKey:[NSString stringWithUTF8String:"badKey"]
                returnedLength:&badKeyLength];

  return self;
}
@end

@interface IntCoderObject : NSObject {
@public
  int value;
}
@end

@implementation IntCoderObject
- (instancetype)initWithValue:(int)v {
  self = [super init];
  value = v;
  return self;
}

- (void)encodeWithCoder:(NSCoder *)coder {
  [coder encodeInt:value forKey:[NSString stringWithUTF8String:"value"]];
}

- (instancetype)initWithCoder:(NSCoder *)coder {
  self = [super init];
  value = [coder decodeIntForKey:[NSString stringWithUTF8String:"value"]];
  return self;
}
@end

int test_NSKeyedArchiver_encodeIntForKey() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  int values[] = {12345, 0, -1, -12345, 0x7FFFFFFF, -0x80000000};
  int count = sizeof(values) / sizeof(int);

  for (int i = 0; i < count; i++) {
    IntCoderObject *obj = [[IntCoderObject alloc] initWithValue:values[i]];
    NSData *archivedData = [NSKeyedArchiver archivedDataWithRootObject:obj];
    IntCoderObject *unarchivedObj =
        [NSKeyedUnarchiver unarchiveObjectWithData:archivedData];

    if (unarchivedObj->value != values[i]) {
      [pool drain];
      return -(i + 1);
    }
  }

  [pool drain];
  return 0;
}

int test_NSKeyedArchiver_NSKeyedUnarchiver() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  char buffer[100];
  for (char i = 0; i < 100; i++) {
    buffer[i] = i;
  }
  CharBufferObject *obj = [[CharBufferObject alloc] initWithBytes:buffer
                                                           length:100];
  NSData *archivedData = [NSKeyedArchiver archivedDataWithRootObject:obj];
  CharBufferObject *unarchivedObj =
      [NSKeyedUnarchiver unarchiveObjectWithData:archivedData];
  if (unarchivedObj->length != obj->length) {
    return -1;
  }
  if (memcmp(unarchivedObj->buffer, obj->buffer, 100) != 0) {
    return -2;
  }
  if (unarchivedObj->badKeyLength != 0) {
    return -3;
  }
  if (unarchivedObj->badKeyBuffer != NULL) {
    return -4;
  }
  [pool drain];
  return 0;
}

int test_NSJSONSerialization_writing() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  NSString *text = [NSString stringWithUTF8String:"a\"\n"];
  NSArray *array =
      [NSArray arrayWithObjects:text, [NSNumber numberWithInt:-7],
                                [NSNumber numberWithBool:true], nil];
  id error = nil;
  NSData *data = [NSJSONSerialization dataWithJSONObject:array
                                                 options:0
                                                   error:&error];
  const char *expected = "[\"a\\\"\\n\",-7,true]";
  int result = data == nil || [data length] != strlen(expected) ||
                       memcmp([data bytes], expected, strlen(expected)) != 0
                   ? -1
                   : 0;
  id keys[] = {[NSString stringWithUTF8String:"items"]};
  id values[] = {array};
  NSDictionary *dict = [NSDictionary dictionaryWithObjects:values
                                                   forKeys:keys
                                                     count:1];
  data = [NSJSONSerialization dataWithJSONObject:dict options:0 error:&error];
  const char *expected_dict = "{\"items\":[\"a\\\"\\n\",-7,true]}";
  if (data == nil || [data length] != strlen(expected_dict) ||
      memcmp([data bytes], expected_dict, strlen(expected_dict)) != 0) {
    result = -3;
  }
  NSArray *bad =
      [NSArray arrayWithObjects:[NSNumber numberWithFloat:INFINITY], nil];
  if ([NSJSONSerialization isValidJSONObject:bad] ||
      [NSJSONSerialization isValidJSONObject:text]) {
    result = -2;
  }
  [pool drain];
  return result;
}

int test_NSDictionary_nil_source() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  NSDictionary *dict = [[NSDictionary alloc] initWithDictionary:nil];
  NSMutableDictionary *mutable =
      [[NSMutableDictionary alloc] initWithDictionary:nil];
  int result =
      dict == nil || mutable == nil || [dict count] != 0 || [mutable count] != 0
          ? -1
          : 0;
  NSString *key = [NSString stringWithUTF8String:"key"];
  NSString *value = [NSString stringWithUTF8String:"value"];
  [mutable setObject:value forKey:key];
  if (![[mutable objectForKey:key] isEqualToString:value]) {
    result = -2;
  }
  [dict release];
  [mutable release];
  [pool drain];
  return result;
}

int test_NSKeyedArchiver_NSDictionary_of_NSArray_of_NSStrings() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  NSArray *fruits =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"apple"],
                                [NSString stringWithUTF8String:"banana"],
                                [NSString stringWithUTF8String:"cherry"], nil];
  NSArray *colors =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"red"],
                                [NSString stringWithUTF8String:"green"],
                                [NSString stringWithUTF8String:"blue"], nil];
  NSArray *values = [NSArray arrayWithObjects:fruits, colors, nil];
  NSArray *keys =
      [NSArray arrayWithObjects:[NSString stringWithUTF8String:"fruits"],
                                [NSString stringWithUTF8String:"colors"], nil];
  NSDictionary *dict = [NSDictionary dictionaryWithObjects:values forKeys:keys];

  NSData *archivedData = [NSKeyedArchiver archivedDataWithRootObject:dict];
  NSDictionary *unarchivedDict =
      [NSKeyedUnarchiver unarchiveObjectWithData:archivedData];

  if (![unarchivedDict isKindOfClass:[NSDictionary class]]) {
    [pool drain];
    return -1;
  }
  if ([unarchivedDict count] != [dict count]) {
    [pool drain];
    return -2;
  }
  if (![unarchivedDict isEqualToDictionary:dict]) {
    [pool drain];
    return -3;
  }

  NSArray *unarchivedFruits =
      [unarchivedDict objectForKey:[NSString stringWithUTF8String:"fruits"]];
  if (![unarchivedFruits isKindOfClass:[NSArray class]]) {
    [pool drain];
    return -4;
  }
  if (![unarchivedFruits isEqualToArray:fruits]) {
    [pool drain];
    return -5;
  }

  NSArray *unarchivedColors =
      [unarchivedDict objectForKey:[NSString stringWithUTF8String:"colors"]];
  if (![unarchivedColors isKindOfClass:[NSArray class]]) {
    [pool drain];
    return -6;
  }
  if (![unarchivedColors isEqualToArray:colors]) {
    [pool drain];
    return -7;
  }

  for (NSUInteger i = 0; i < [unarchivedFruits count]; i++) {
    if (![[unarchivedFruits objectAtIndex:i] isKindOfClass:[NSString class]]) {
      [pool drain];
      return -8;
    }
  }

  [pool drain];
  return 0;
}

int test_NSNumber_stringValue() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  NSNumber *num;
  NSString *result;
  NSString *expected;

  // Bool: YES -> "1"
  num = [NSNumber numberWithBool:YES];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"1"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -1;
  }

  // Bool: NO -> "0"
  num = [NSNumber numberWithBool:NO];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -2;
  }

  // Int: 0 -> "0"
  num = [NSNumber numberWithInt:0];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -3;
  }

  // Int: 42 -> "42"
  num = [NSNumber numberWithInt:42];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"42"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -4;
  }

  // Int: -100 -> "-100"
  num = [NSNumber numberWithInt:-100];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-100"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -5;
  }

  // Int: INT_MAX -> "2147483647"
  num = [NSNumber numberWithInt:2147483647];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"2147483647"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -6;
  }

  // Int: INT_MIN -> "-2147483648"
  num = [NSNumber numberWithInt:-2147483648];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-2147483648"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -7;
  }

  // LongLong: 0 -> "0"
  num = [NSNumber numberWithLongLong:0LL];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -8;
  }

  // LongLong: 9999999999 -> "9999999999"
  num = [NSNumber numberWithLongLong:9999999999LL];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"9999999999"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -9;
  }

  // LongLong: -9999999999 -> "-9999999999"
  num = [NSNumber numberWithLongLong:-9999999999LL];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-9999999999"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -10;
  }

  // UnsignedInt: 0 -> "0"
  num = [NSNumber numberWithUnsignedInt:0U];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -11;
  }

  // UnsignedLongLong: 0 -> "0"
  num = [NSNumber numberWithUnsignedLongLong:0ULL];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -12;
  }

  // Float: 0.0 -> "0"
  num = [NSNumber numberWithFloat:0.0f];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -13;
  }

  // Float: 1.5 -> "1.5"
  num = [NSNumber numberWithFloat:1.5f];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"1.5"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -14;
  }

  // Float: -1.25 -> "-1.25"
  num = [NSNumber numberWithFloat:-1.25f];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-1.25"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -15;
  }

  // Double: 0.0 -> "0"
  num = [NSNumber numberWithDouble:0.0];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -16;
  }

  // Double: 1.5 -> "1.5"
  num = [NSNumber numberWithDouble:1.5];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"1.5"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -17;
  }

  // Double: -1.25 -> "-1.25"
  num = [NSNumber numberWithDouble:-1.25];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-1.25"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -18;
  }

  // Short: 0 -> "0"
  num = [NSNumber numberWithShort:0];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -19;
  }

  // Short: 100 -> "100"
  num = [NSNumber numberWithShort:100];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"100"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -20;
  }

  // Short: SHRT_MIN -> "-32768"
  num = [NSNumber numberWithShort:-32768];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-32768"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -21;
  }

  // Short: SHRT_MAX -> "32767"
  num = [NSNumber numberWithShort:32767];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"32767"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -22;
  }

  // UnsignedShort: 0 -> "0"
  num = [NSNumber numberWithUnsignedShort:0];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -23;
  }

  // Char: 0 -> "0"
  num = [NSNumber numberWithChar:0];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"0"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -24;
  }

  // Char: 65 -> "65"
  num = [NSNumber numberWithChar:65];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"65"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -25;
  }

  // Char: SCHAR_MIN -> "-128"
  num = [NSNumber numberWithChar:-128];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"-128"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -26;
  }

  // Char: SCHAR_MAX -> "127"
  num = [NSNumber numberWithChar:127];
  result = [num stringValue];
  expected = [NSString stringWithUTF8String:"127"];
  if (![result isEqualToString:expected]) {
    [pool drain];
    return -27;
  }

  [pool drain];
  return 0;
}

@interface NotificationObserver : NSObject {
@public
  int receivedCount;
  id lastNotification;
}
- (void)handleNotification:(NSNotification *)notification;
@end

@implementation NotificationObserver
- (void)handleNotification:(NSNotification *)notification {
  receivedCount++;
  [lastNotification release];
  lastNotification = [notification retain];
}
- (void)dealloc {
  [lastNotification release];
  [super dealloc];
}
@end

// When name is nil, the observer should receive notifications of any name.
int test_NSNotificationCenter_addObserver_nilName() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
  NotificationObserver *observer = [NotificationObserver new];
  SEL sel = NSSelectorFromString(
      [NSString stringWithUTF8String:"handleNotification:"]);

  [center addObserver:observer selector:sel name:nil object:nil];

  [center postNotificationName:[NSString stringWithUTF8String:"FirstName"]
                        object:nil];
  if (observer->receivedCount != 1) {
    [center removeObserver:observer];
    [observer release];
    [pool drain];
    return -1;
  }

  [center postNotificationName:[NSString stringWithUTF8String:"SecondName"]
                        object:nil];
  if (observer->receivedCount != 2) {
    [center removeObserver:observer];
    [observer release];
    [pool drain];
    return -2;
  }

  // The last notification's name should match the most recently posted one.
  NSString *lastName = [observer->lastNotification name];
  NSString *expectedName = [NSString stringWithUTF8String:"SecondName"];
  if (![lastName isEqualToString:expectedName]) {
    [center removeObserver:observer];
    [observer release];
    [pool drain];
    return -3;
  }

  [center removeObserver:observer];
  [observer release];
  [pool drain];
  return 0;
}

// When name is nil but object is specified, only notifications from that
// sender (with any name) should be delivered to the observer.
int test_NSNotificationCenter_addObserver_nilName_withObject() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
  NotificationObserver *observer = [NotificationObserver new];
  SEL sel = NSSelectorFromString(
      [NSString stringWithUTF8String:"handleNotification:"]);

  NSObject *sender = [NSObject new];
  NSObject *otherSender = [NSObject new];

  [center addObserver:observer selector:sel name:nil object:sender];

  // Notification from the matching sender should be delivered, regardless of
  // the notification's name.
  [center postNotificationName:[NSString stringWithUTF8String:"AnyName"]
                        object:sender];
  if (observer->receivedCount != 1) {
    [center removeObserver:observer];
    [sender release];
    [otherSender release];
    [observer release];
    [pool drain];
    return -1;
  }

  // Notification from a different sender should be filtered out, even though
  // name is nil.
  [center postNotificationName:[NSString stringWithUTF8String:"AnyName"]
                        object:otherSender];
  if (observer->receivedCount != 1) {
    [center removeObserver:observer];
    [sender release];
    [otherSender release];
    [observer release];
    [pool drain];
    return -2;
  }

  // A different notification name from the matching sender should still be
  // delivered.
  [center postNotificationName:[NSString stringWithUTF8String:"OtherName"]
                        object:sender];
  if (observer->receivedCount != 2) {
    [center removeObserver:observer];
    [sender release];
    [otherSender release];
    [observer release];
    [pool drain];
    return -3;
  }

  [center removeObserver:observer];
  [sender release];
  [otherSender release];
  [observer release];
  [pool drain];
  return 0;
}

// An observer registered with name=nil should be properly unregistered by
// removeObserver:, so it must not receive any further notifications.
int test_NSNotificationCenter_addObserver_nilName_removeObserver() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
  NotificationObserver *observer = [NotificationObserver new];
  SEL sel = NSSelectorFromString(
      [NSString stringWithUTF8String:"handleNotification:"]);

  [center addObserver:observer selector:sel name:nil object:nil];

  [center postNotificationName:[NSString stringWithUTF8String:"BeforeRemove"]
                        object:nil];
  if (observer->receivedCount != 1) {
    [center removeObserver:observer];
    [observer release];
    [pool drain];
    return -1;
  }

  [center removeObserver:observer];

  [center postNotificationName:[NSString stringWithUTF8String:"AfterRemove"]
                        object:nil];
  if (observer->receivedCount != 1) {
    [observer release];
    [pool drain];
    return -2;
  }

  // Posting under a different name after removal should not deliver either.
  [center postNotificationName:[NSString stringWithUTF8String:"OtherName"]
                        object:nil];
  if (observer->receivedCount != 1) {
    [observer release];
    [pool drain];
    return -3;
  }

  [observer release];
  [pool drain];
  return 0;
}

int test_NSSet_setWithObjects_count() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Duplicate objects are only included once.
  id one = @1, two = @2;
  id objects[] = {one, two, one};
  NSSet *set = [NSSet setWithObjects:objects count:3];
  if ([set count] != 2 || ![set containsObject:@1] ||
      ![set containsObject:@2]) {
    [pool drain];
    return -1;
  }

  // The mutable subclass gives a mutable set.
  NSMutableSet *mutableSet = [NSMutableSet setWithObjects:objects count:2];
  [mutableSet addObject:@3];
  if ([mutableSet count] != 3 || ![mutableSet containsObject:@3]) {
    [pool drain];
    return -2;
  }

  [pool drain];
  return 0;
}

int test_NSMutableArray_removeObjectsInArray_otherArray() {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];

  // Duplicate objects are removed as well.
  NSMutableArray *array =
      [[NSMutableArray alloc] initWithObjects:@1, @2, @3, @4, @5, @2, nil];
  NSArray *otherArray = [[NSArray alloc] initWithObjects:@2, @4, @99, nil];
  [array removeObjectsInArray:otherArray];
  [otherArray release];

  if ([array count] != 3) {
    [array release];
    [pool drain];
    return -1;
  }

  // Expected: [1, 3, 5]
  NSArray *expectedArray = [[NSArray alloc] initWithObjects:@1, @3, @5, nil];
  if (![array isEqualToArray:expectedArray]) {
    [expectedArray release];
    [array release];
    [pool drain];
    return -2;
  }

  [expectedArray release];
  [array release];
  [pool drain];
  return 0;
}

int test_malloc_zone_basic() {
  malloc_zone_t *zone = malloc_create_zone(0, 0);
  unsigned char *p = malloc_zone_malloc(zone, 128);
  if (zone->size(zone, p) != 128) {
    return -1;
  }

  memset(p, 0xAB, 128);
  for (int i = 0; i < 128; i++) {
    if (p[i] != 0xAB) {
      malloc_zone_free(zone, p);
      malloc_destroy_zone(zone);
      return -2;
    }
  }
  malloc_zone_free(zone, p);
  malloc_destroy_zone(zone);

  return 0;
}

int test_malloc_zone_struct_dispatch() {
  malloc_zone_t *zone = malloc_default_zone();
  if (!zone)
    return -1;

  void *p = zone->malloc(zone, 128);
  if (!p)
    return -2;

  // malloc_size() uses the default zone. If the allocation did not work
  // this should cause a panic and thus fail the test.
  size_t sz = malloc_size(p);
  if (sz != 128) {
    zone->free(zone, p);
    return -3;
  }

  zone->free(zone, p);
  return 0;
}

int test_malloc_zone_extended_dispatch() {
  malloc_zone_t *zones[2] = {malloc_default_zone(), malloc_create_zone(0, 0)};
  for (int i = 0; i < 2; i++) {
    malloc_zone_t *zone = zones[i];
    if (!zone || zone->version < 5 || !zone->calloc || !zone->valloc ||
        !zone->memalign)
      return 1;
    unsigned char *a = zone->calloc(zone, 17, 3);
    unsigned char *b = zone->valloc(zone, 5001);
    void *c = zone->memalign(zone, 65536, 17);
    if (!a || !b || !c || ((uintptr_t)b & 4095) || ((uintptr_t)c & 65535))
      return 2;
    for (int j = 0; j < 51; j++)
      if (a[j])
        return 3;
    for (int j = 0; j < 5001; j++)
      if (b[j])
        return 4;
    if (malloc_zone_from_ptr(a) != zone || malloc_zone_from_ptr(b) != zone ||
        malloc_zone_from_ptr(c) != zone || zone->size(zone, c) < 17)
      return 5;
    zone->free(zone, a);
    zone->free(zone, b);
    zone->free(zone, c);
    zone->free(zone, NULL);
    a = malloc_zone_calloc(zone, 5, 7);
    b = malloc_zone_valloc(zone, 1);
    c = malloc_zone_memalign(zone, 8192, 1);
    if (!a || !b || !c || ((uintptr_t)b & 4095) || ((uintptr_t)c & 8191))
      return 6;
    free(a);
    free(b);
    free(c);
  }
  zones[1]->destroy(zones[1]);
  return 0;
}

int test_malloc_zone_ownership_and_destruction() {
  malloc_zone_t *a = malloc_create_zone(4096, 0);
  malloc_zone_t *b = malloc_create_zone(4096, 0);
  if (!a || !b || a == b)
    return 1;
  unsigned char *p = malloc_zone_malloc(a, 32);
  unsigned char *q = malloc_zone_memalign(b, 8192, 17);
  if (!p || !q || a->size(a, q) || b->size(b, p) ||
      malloc_zone_from_ptr(p) != a || malloc_zone_from_ptr(q) != b)
    return 2;
  memset(p, 0x5a, 32);
  memset(q, 0x33, 17);
  if (malloc_size(p) < 32 || malloc_size(q) < 17)
    return 3;
  p = realloc(p, 1024);
  if (!p || malloc_zone_from_ptr(p) != a)
    return 4;
  for (int i = 0; i < 32; i++)
    if (p[i] != 0x5a)
      return 5;
  void *(*volatile resize)(void *, size_t) = realloc;
  errno = 0;
  if (resize(p, (size_t)-1) || errno != ENOMEM || a->size(a, p) < 1024)
    return 6;
  void *(*volatile resize_free)(void *, size_t) = reallocf;
  if (resize_free(p, (size_t)-1) || errno != ENOMEM || malloc_zone_from_ptr(p))
    return 7;
  void *live = malloc_zone_malloc(a, 64);
  void *aligned = malloc_zone_memalign(a, 65536, 17);
  if (!live || !aligned)
    return 8;
  malloc_destroy_zone(a);
  if (malloc_zone_from_ptr(live) || malloc_zone_from_ptr(aligned) ||
      malloc_zone_from_ptr(q) != b)
    return 9;
  for (int i = 0; i < 17; i++)
    if (q[i] != 0x33)
      return 10;
  free(q);
  if (malloc_zone_from_ptr(q))
    return 11;
  malloc_destroy_zone(b);
  int stack_value = 0;
  return malloc_zone_from_ptr(NULL) || malloc_zone_from_ptr(&stack_value) ? 12
                                                                          : 0;
}

int test_malloc_zone_names() {
  malloc_zone_t *zone = malloc_create_zone(0, 0);
  if (!zone)
    return 1;
  char name[] = "first";
  malloc_set_zone_name(zone, name);
  name[0] = 'X';
  const char *first = malloc_get_zone_name(zone);
  if (!first || strcmp(first, "first"))
    return 2;
  // The current name is a valid input; copy before freeing its old storage.
  malloc_set_zone_name(zone, first);
  const char *second = malloc_get_zone_name(zone);
  if (!second || strcmp(second, "first") || malloc_zone_from_ptr(first))
    return 3;
  malloc_set_zone_name(zone, NULL);
  if (malloc_get_zone_name(zone) || malloc_zone_from_ptr(second))
    return 4;
  malloc_set_zone_name(zone, "last");
  const char *last = malloc_get_zone_name(zone);
  if (!last || strcmp(last, "last"))
    return 5;
  malloc_destroy_zone(zone);
  return malloc_zone_from_ptr(last) ? 6 : 0;
}

int test_posix_memalign_contracts() {
  int (*volatile aligned)(void **, size_t, size_t) = posix_memalign;
  size_t alignments[] = {4, 8, 16, 32, 64, 4096, 65536, 1048576};
  for (unsigned i = 0; i < sizeof(alignments) / sizeof(alignments[0]); i++) {
    for (size_t size = 0; size < 18; size += 17) {
      void *p = NULL;
      errno = EDOM;
      if (aligned(&p, alignments[i], size) || !p ||
          ((uintptr_t)p & (alignments[i] - 1)) || malloc_size(p) < size ||
          errno != EDOM)
        return 1;
      if (size) {
        memset(p, 0x5a, size);
        p = realloc(p, 32768);
        if (!p)
          return 2;
        for (size_t j = 0; j < size; j++)
          if (((unsigned char *)p)[j] != 0x5a)
            return 3;
      }
      free(p);
    }
  }
  size_t invalid[] = {0, 1, 2, 3, 6, 24};
  for (unsigned i = 0; i < sizeof(invalid) / sizeof(invalid[0]); i++) {
    void *p = (void *)0x1234;
    errno = EDOM;
    if (aligned(&p, invalid[i], 17) != EINVAL || p != (void *)0x1234 ||
        errno != EDOM)
      return 4;
  }
  void *p = (void *)0x1234;
  errno = EDOM;
  return aligned(&p, 65536, (size_t)-1) != ENOMEM || p != (void *)0x1234 ||
                 errno != EDOM
             ? 5
             : 0;
}

int test_malloc_zone_allocation_errors() {
  malloc_zone_t *zone = malloc_create_zone(0, 0);
  if (!zone)
    return 1;
  errno = 0;
  if (zone->malloc(zone, (size_t)-1) || errno != ENOMEM)
    return 2;
  errno = 0;
  if (zone->calloc(zone, 0x80000001u, 2) || errno != ENOMEM)
    return 3;
  errno = 0;
  if (zone->valloc(zone, (size_t)-1) || errno != ENOMEM)
    return 4;
  errno = 0;
  if (zone->memalign(zone, 3, 0) || errno != EINVAL)
    return 5;
  if (zone->memalign(zone, 65536, (size_t)-1) || errno != ENOMEM)
    return 6;
  if (malloc_create_zone((size_t)-1, 0) || errno != ENOMEM)
    return 7;
  void *p = zone->realloc(zone, NULL, 16);
  void *zero = zone->realloc(zone, p, 0);
  if (!zero || zero == p || malloc_zone_from_ptr(p) ||
      malloc_zone_from_ptr(zero) != zone)
    return 8;
  zone->free(zone, zero);
  malloc_destroy_zone(zone);
  return 0;
}

int test_fcvt() {
  char *buf;
  int dec, sign;

  buf = fcvt(123.4567, 6, &dec, &sign);
  if (strcmp(buf, "123456700") != 0 || dec != 3 || sign != 0) {
    return -1;
  }

  buf = fcvt(123.4567, 2, &dec, &sign);
  if (strcmp(buf, "12346") != 0 || dec != 3 || sign != 0) {
    return -2;
  }

  buf = fcvt(0, 2, &dec, &sign);
  if (strcmp(buf, "00") != 0 || dec != 0 || sign != 0) {
    return -3;
  }

  buf = fcvt(6.491250038146973, 2, &dec, &sign);
  if (strcmp(buf, "649") != 0 || dec != 1 || sign != 0) {
    return -4;
  }

  buf = fcvt(-9.876, 10, &dec, &sign);
  if (strcmp(buf, "98760000000") != 0 || dec != 1 || sign != 1) {
    return -5;
  }

  buf = fcvt(0.125, 3, &dec, &sign);
  if (strcmp(buf, "125") != 0 || dec != 0 || sign != 0) {
    return -6;
  }

  buf = fcvt(0.00125, 5, &dec, &sign);
  if (strcmp(buf, "125") != 0 || dec != -2 || sign != 0) {
    return -7;
  }

  buf = fcvt(-0.00125, 5, &dec, &sign);
  if (strcmp(buf, "125") != 0 || dec != -2 || sign != 1) {
    return -8;
  }

  buf = fcvt(0.0001, 2, &dec, &sign);
  if (strcmp(buf, "") != 0 || dec != -2 || sign != 0) {
    return -9;
  }

  buf = fcvt(-0.0001, 2, &dec, &sign);
  if (strcmp(buf, "") != 0 || dec != -2 || sign != 1) {
    return -10;
  }

  buf = fcvt(1e-300, 5, &dec, &sign);
  if (strcmp(buf, "") != 0 || dec != -5 || sign != 0) {
    return -11;
  }

  buf = fcvt(0.004, 2, &dec, &sign);
  if (strcmp(buf, "") != 0 || dec != -2 || sign != 0) {
    return -12;
  }

  buf = fcvt(0.005, 2, &dec, &sign);
  if (strcmp(buf, "1") != 0 || dec != -1 || sign != 0) {
    return -13;
  }

  buf = fcvt(0.006, 2, &dec, &sign);
  if (strcmp(buf, "1") != 0 || dec != -1 || sign != 0) {
    return -14;
  }

  return 0;
}

#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
// touchHLE stores temporary files in the app's sandbox. On macOS, tmpfile()
// uses a shared directory whose contents may change independently of this test.
static int tempfile_directory_entry_count(void) {
  const char *home = getenv("HOME");
  if (home == NULL)
    return -1;

  char tmp_dir[1024];
  int len = snprintf(tmp_dir, sizeof(tmp_dir), "%s/tmp", home);
  if (len < 0 || (size_t)len >= sizeof(tmp_dir))
    return -1;

  DIR *dir = opendir(tmp_dir);
  if (dir == NULL)
    return -1;

  int count = 0;
  errno = 0;
  while (readdir(dir) != NULL)
    count++;
  int read_error = errno;
  if (closedir(dir) != 0 || read_error != 0)
    return -1;
  return count;
}
#endif

int test_tempfile() {
#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
  int entries_before = tempfile_directory_entry_count();
  if (entries_before < 0)
    return -1;
#endif

  FILE *files[2] = {NULL, NULL};
  const unsigned char contents[2][5] = {{'a', '\0', '\n', 0xff, 'z'},
                                        {'b', '\0', '\n', 0xfe, 'y'}};
  unsigned char buffer[sizeof(contents[0])];
  int result = 0;

  for (int i = 0; i < 2; i++) {
    files[i] = tmpfile();
    if (files[i] == NULL) {
      result = -2;
      goto cleanup;
    }
    // Each stream starts empty and positioned at its beginning.
    if (ftell(files[i]) != 0 || fgetc(files[i]) != EOF || !feof(files[i])) {
      result = -3;
      goto cleanup;
    }
    if (fseek(files[i], 0, SEEK_SET) != 0 || feof(files[i])) {
      result = -4;
      goto cleanup;
    }
    // Flush before creating the next stream so buffered I/O cannot hide an
    // accidental reuse or truncation of the first file.
    if (fwrite(contents[i], 1, sizeof(contents[i]), files[i]) !=
            sizeof(contents[i]) ||
        fflush(files[i]) != 0) {
      result = -5;
      goto cleanup;
    }
  }

  for (int i = 0; i < 2; i++) {
    if (fseek(files[i], 0, SEEK_SET) != 0 ||
        fread(buffer, 1, sizeof(buffer), files[i]) != sizeof(buffer) ||
        memcmp(buffer, contents[i], sizeof(buffer)) != 0) {
      result = -6;
      goto cleanup;
    }
    if (fgetc(files[i]) != EOF || !feof(files[i])) {
      result = -7;
      goto cleanup;
    }
    // Closing the first stream must leave the second stream usable.
    int close_result = fclose(files[i]);
    files[i] = NULL;
    if (close_result != 0) {
      result = -8;
      goto cleanup;
    }
  }

cleanup:
  for (int i = 0; i < 2; i++) {
    if (files[i] != NULL)
      fclose(files[i]);
  }
#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
  if (result == 0 && tempfile_directory_entry_count() != entries_before)
    result = -9;
#endif
  return result;
}

int test_scanf_leading_zero() {
  unsigned int value = 99;
  int wheel = -1;
  float scale = -1;
  if (sscanf("0 254 0.8", "%x %d %f", &value, &wheel, &scale) != 3 ||
      value != 0 || wheel != 254 || fabsf(scale - 0.8f) > 0.0001f)
    return -1;
  if (sscanf("0Z", "%1x", &value) != 1 || value != 0)
    return -2;
  int next = -1;
  if (sscanf("0 17", "%i %d", &wheel, &next) != 2 || wheel != 0 || next != 17)
    return -3;
  FILE *file = tmpfile();
  if (file == NULL)
    return -4;
  fputs("0 254 0.8", file);
  rewind(file);
  int result = 0;
  if (fscanf(file, "%x %d %f", &value, &wheel, &scale) != 3 || value != 0 ||
      wheel != 254 || fabsf(scale - 0.8f) > 0.0001f)
    result = -5;
  fclose(file);
  return result;
}

int test_scanf_characters() {
  char buffer[3] = {'?', '?', '!'};
  if (sscanf(" \nX", "%2c", buffer) != 1 || buffer[0] != ' ' ||
      buffer[1] != '\n' || buffer[2] != '!')
    return -1;
  buffer[0] = '?';
  if (sscanf("", "%c", buffer) != EOF || buffer[0] != '?')
    return -2;
  if (sscanf("Z", "%c", buffer) != 1 || buffer[0] != 'Z' || buffer[2] != '!')
    return -3;
  FILE *file = tmpfile();
  if (file == NULL)
    return -4;
  const char bytes[2] = {0, 'Z'};
  fwrite(bytes, 1, 2, file);
  rewind(file);
  int result = 0;
  if (fscanf(file, "%2c", buffer) != 1 || buffer[0] != 0 || buffer[1] != 'Z' ||
      buffer[2] != '!')
    result = -5;
  fclose(file);
  return result;
}

int test_sscanf_string_input_end() {
  char first[16] = "unchanged";
  char second[16] = "untouched";
  int value = 0;
  if (sscanf("", "%s", first) != EOF || strcmp(first, "unchanged") != 0)
    return -1;
  if (sscanf(" \t\n", "%s", first) != EOF || strcmp(first, "unchanged") != 0)
    return -2;
  if (sscanf("42", "%d %s", &value, first) != 1 || value != 42 ||
      strcmp(first, "unchanged") != 0)
    return -3;
  if (sscanf("ab", "%2s %s", first, second) != 1 || strcmp(first, "ab") != 0 ||
      strcmp(second, "untouched") != 0)
    return -4;
  return 0;
}

int test_fread_partial_eof() {
  FILE *file = tmpfile();
  if (file == NULL)
    return -1;
  const char contents[] = "abcde";
  char buffer[8];
  memset(buffer, 0x55, sizeof(buffer));
  int result = 0;
  if (fwrite(contents, 1, 5, file) != 5 || fseek(file, 0, SEEK_SET) != 0) {
    result = -2;
  } else if (fread(buffer, 2, 4, file) != 2 || !feof(file) || ferror(file) ||
             memcmp(buffer, contents, 5) != 0 || buffer[5] != 0x55) {
    result = -3;
  } else if (fseek(file, 0, SEEK_SET) != 0 || feof(file) ||
             fread(buffer, 1, 5, file) != 5 || feof(file) ||
             fread(buffer, 1, 1, file) != 0 || !feof(file)) {
    result = -4;
  }
  fclose(file);
  return result;
}

// Exercise the compiler's 32/64-bit Blocks ABI without requiring -fblocks.
extern void *_NSConcreteStackBlock[];
extern void *_Block_copy(const void *);
extern void _Block_release(const void *);

struct TestBlock;
struct TestBlockDescriptor {
  unsigned long reserved;
  unsigned long size;
  void (*copy)(void *, const void *);
  void (*dispose)(const void *);
};
struct TestBlock {
  void *isa;
  int flags;
  int reserved;
  void (*invoke)(struct TestBlock *);
  const struct TestBlockDescriptor *descriptor;
  int captured;
  int *output;
};
static int block_copy_calls;
static int block_dispose_calls;
static void test_block_copy_helper(void *dest, const void *src) {
  block_copy_calls++;
}
static void test_block_dispose_helper(const void *block) {
  block_dispose_calls++;
}
static void test_block_invoke(struct TestBlock *block) {
  *block->output = block->captured;
}

int test_Block_copy_lifetime() {
  static const struct TestBlockDescriptor descriptor = {
      0, sizeof(struct TestBlock), test_block_copy_helper,
      test_block_dispose_helper};
  int output = 0;
  struct TestBlock local = {
      _NSConcreteStackBlock, 1 << 25, 0,      test_block_invoke,
      &descriptor,           71,      &output};
  block_copy_calls = 0;
  block_dispose_calls = 0;
  struct TestBlock *copy = _Block_copy(&local);
  if (copy == &local || block_copy_calls != 1)
    return -1;
  local.captured = -1;
  struct TestBlock *second = _Block_copy(copy);
  if (second != copy || block_copy_calls != 1)
    return -2;
  _Block_release(copy);
  if (block_dispose_calls != 0)
    return -3;
  second->invoke(second);
  if (output != 71)
    return -4;
  _Block_release(second);
  return block_dispose_calls == 1 ? 0 : -5;
}

// clang-format off
#define FUNC_DEF(func)                                                         \
  { &func, #func }
struct {
  int (*func)();
  const char *name;
} test_func_array[] = {
#ifndef DEFINE_ME_WHEN_BUILDING_ON_MACOS
    // below tests are failing on macOS,
    // so we skip them
    FUNC_DEF(test_getcwd_chdir),
    FUNC_DEF(test_synchronized),
    FUNC_DEF(test_read_directory_as_fd),
    FUNC_DEF(test_pthread_get_stacksize_np),
    FUNC_DEF(test_cpp_virtual_inheritance),
#endif
    FUNC_DEF(test_qsort),
    FUNC_DEF(test_vsnprintf),
    FUNC_DEF(test_variadic_callback_snapshot),
    FUNC_DEF(test_uuid_c_values),
    FUNC_DEF(test_named_semaphore_reopen),
    FUNC_DEF(test_semaphore_lifecycle_and_unlink),
    FUNC_DEF(test_semaphore_error_boundaries),
    FUNC_DEF(test_semaphore_counter_limits),
    FUNC_DEF(test_dispatch_semaphore_independent_limit),
    FUNC_DEF(test_semaphore_waiter_cleanup),
    FUNC_DEF(test_nsuuid_values),
    FUNC_DEF(test_cfuuid_byte_abi),
    FUNC_DEF(test_cf_copy_description_ownership),
    FUNC_DEF(test_cfuuid_string_and_reference_lifetimes),
    FUNC_DEF(test_cfuuid_constants),
    FUNC_DEF(test_cfuuid_historical_parser),
    FUNC_DEF(test_cfdictionary_callback_lifecycle),
    FUNC_DEF(test_cfdictionary_replaces_equal_key),
    FUNC_DEF(test_cfdictionary_create_and_copy_callbacks),
    FUNC_DEF(test_cfdictionary_apply_function),
    FUNC_DEF(test_cfdictionary_presence_and_collision),
    FUNC_DEF(test_cfdictionary_contains_value_callbacks),
    FUNC_DEF(test_cfdata_copy_and_empty_range),
    FUNC_DEF(test_cflocale_copy_lifetime),
    FUNC_DEF(test_cfurl_copy_scheme),
    FUNC_DEF(test_cfurl_port_number),
    FUNC_DEF(test_cfurl_host_name),
    FUNC_DEF(test_cfurl_create_bytes),
    FUNC_DEF(test_cfurl_directory_path),
    FUNC_DEF(test_cfurl_escaped_path),
    FUNC_DEF(test_cfurl_query_and_fragment),
    FUNC_DEF(test_cfurl_legacy_percent_decode),
    FUNC_DEF(test_cfdata_no_copy_deallocator),
    FUNC_DEF(test_cfdata_equality_and_dictionary_keys),
    FUNC_DEF(test_cfdata_mutable_copy_and_length),
    FUNC_DEF(test_cfdata_replace_and_delete),
    FUNC_DEF(test_cfarray_custom_callbacks),
    FUNC_DEF(test_cfarray_nonretaining_mutations),
    FUNC_DEF(test_nsmutablearray_remove_multiple_matches),
    FUNC_DEF(test_nsmutabledictionary_replacement_key_lifetime),
    FUNC_DEF(test_cfuuid_cf_collection_interop),
    FUNC_DEF(test_cfstring_character_creation),
    FUNC_DEF(test_sscanf),
    FUNC_DEF(test_swscanf),
    FUNC_DEF(test_realloc),
    FUNC_DEF(test_allocation_failure_contracts),
    FUNC_DEF(test_allocation_zero_and_resize_contracts),
    FUNC_DEF(test_allocation_errno_is_thread_local),
    FUNC_DEF(test_valloc),
    FUNC_DEF(test_atof),
    FUNC_DEF(test_strtof),
    FUNC_DEF(test_sem),
    FUNC_DEF(test_mtsem),
    FUNC_DEF(test_thread_suspend_resume),
    FUNC_DEF(test_CGAffineTransform),
    FUNC_DEF(test_strncpy),
    FUNC_DEF(test_strncat),
    FUNC_DEF(test_bounded_string_extensions),
    FUNC_DEF(test_binary_substring_extensions),
    FUNC_DEF(test_version_comparison_extensions),
    FUNC_DEF(test_shuffle_extensions),
    FUNC_DEF(test_reentrant_error_message_extensions),
    FUNC_DEF(test_reentrant_error_messages_across_threads),
    FUNC_DEF(test_end_pointer_copy_extensions),
    FUNC_DEF(test_case_and_terminator_search_extensions),
    FUNC_DEF(test_overlap_copy_and_explicit_wipe_extensions),
    FUNC_DEF(test_reentrant_tokenizer_extensions),
    FUNC_DEF(test_reverse_search_and_index_extensions),
    FUNC_DEF(test_strlcpy),
    FUNC_DEF(test_setlocale),
    FUNC_DEF(test_strtoul),
    FUNC_DEF(test_strtol),
    FUNC_DEF(test_wcstol),
    FUNC_DEF(test_dirent),
    FUNC_DEF(test_scandir),
    FUNC_DEF(test_glob),
    FUNC_DEF(test_fnmatch),
    FUNC_DEF(test_strchr),
    FUNC_DEF(test_swprintf),
    FUNC_DEF(test_realpath),
    FUNC_DEF(test_ungetc),
    FUNC_DEF(test_fscanf),
    FUNC_DEF(test_fscanf_new),
    FUNC_DEF(test_CFStringFind),
    FUNC_DEF(test_strcspn),
    FUNC_DEF(test_mbstowcs),
    FUNC_DEF(test_CFMutableString),
    FUNC_DEF(test_fwrite),
    FUNC_DEF(test_flockfile_basic),
    FUNC_DEF(test_flockfile_recursive),
    FUNC_DEF(test_ftrylockfile_unlocked),
    FUNC_DEF(test_ftrylockfile_locked_by_other_thread),
    FUNC_DEF(test_flockfile_blocks_other_thread),
    FUNC_DEF(test_flockfile_io_while_locked),
    FUNC_DEF(test_open),
    FUNC_DEF(test_close),
    FUNC_DEF(test_cond_var),
    FUNC_DEF(test_cond_var_static),
    FUNC_DEF(test_cond_timedwait_signaled_before_timeout),
    FUNC_DEF(test_cond_timedwait_past_deadline),
    FUNC_DEF(test_cond_timedwait_broadcast),
    FUNC_DEF(test_cond_timedwait_flag_not_sticky),
    FUNC_DEF(test_cond_timedwait_sibling_not_dropped),
    FUNC_DEF(test_pthread_mutex_normal),
    FUNC_DEF(test_pthread_mutex_recursive_trylock),
    FUNC_DEF(test_NSConditionLock_init),
    FUNC_DEF(test_NSConditionLock_lock_unlock),
    FUNC_DEF(test_NSConditionLock_tryLockWhenCondition),
    FUNC_DEF(test_NSConditionLock_tryLock_contended),
    FUNC_DEF(test_NSConditionLock_producer_consumer),
    FUNC_DEF(test_CFMutableDictionary_NullCallbacks),
    FUNC_DEF(test_CFMutableDictionary_CustomCallbacks_PrimitiveTypes),
    FUNC_DEF(test_CFMutableDictionary_CustomCallbacks_CFTypes),
    FUNC_DEF(test_lrint),
    FUNC_DEF(test_fesetround),
    FUNC_DEF(test_ldexp),
    FUNC_DEF(test_maskrune),
    FUNC_DEF(test_frexpf),
    FUNC_DEF(test_frexp),
    FUNC_DEF(test_setjmp),
    FUNC_DEF(test_inet_addr),
    FUNC_DEF(test_inet_ntop),
    FUNC_DEF(test_inet_pton),
    FUNC_DEF(test_CFURL),
    FUNC_DEF(test_CFNumberCompare_simple),
    FUNC_DEF(test_CFNumberCompare_extended),
    FUNC_DEF(test_memset_pattern),
    FUNC_DEF(test_CGGeometry),
    FUNC_DEF(test_CFURLHasDirectoryPath),
    FUNC_DEF(test_CGImage_invalid_rgb32),
    FUNC_DEF(test_CGImage_JPEG),
    FUNC_DEF(test_NSMutableString_deleteCharactersInRange),
    FUNC_DEF(test_NSString_stringByReplacingOccurrencesOfString),
    FUNC_DEF(test_NSString_stringByReplacingOccurrencesOfString_options_range),
    FUNC_DEF(test_NSString_pathWithComponents),
    FUNC_DEF(test_strptime),
    FUNC_DEF(test_strftime),
    FUNC_DEF(test_wcsftime),
    FUNC_DEF(test_RespondsToSelector),
    FUNC_DEF(test_NSKeyedArchiver_encodeIntForKey),
    FUNC_DEF(test_NSKeyedArchiver_NSKeyedUnarchiver),
    FUNC_DEF(test_NSDictionary_nil_source),
    FUNC_DEF(test_NSJSONSerialization_writing),
    FUNC_DEF(test_NSKeyedArchiver_NSDictionary_of_NSArray_of_NSStrings),
    FUNC_DEF(test_AutoreleasePool),
    FUNC_DEF(test_NSNumber_stringValue),
    FUNC_DEF(test_NSMethodSignature),
    FUNC_DEF(test_NSInvocation),
    FUNC_DEF(test_NSInvocation_invokeWithTarget),
    FUNC_DEF(test_NSInvocation_retainArguments),
    FUNC_DEF(test_NSInvocation_pointer),
    FUNC_DEF(test_Initialize),
    FUNC_DEF(test_NSNotificationCenter_addObserver_nilName),
    FUNC_DEF(test_NSNotificationCenter_addObserver_nilName_withObject),
    FUNC_DEF(test_NSNotificationCenter_addObserver_nilName_removeObserver),
    FUNC_DEF(test_NSMutableArray_removeObjectsInArray_otherArray),
    FUNC_DEF(test_scanf_leading_zero),
    FUNC_DEF(test_scanf_characters),
    FUNC_DEF(test_sscanf_string_input_end),
    FUNC_DEF(test_fread_partial_eof),
    FUNC_DEF(test_Block_copy_lifetime),
    FUNC_DEF(test_malloc_zone_basic),
    FUNC_DEF(test_malloc_zone_struct_dispatch),
    FUNC_DEF(test_malloc_zone_extended_dispatch),
    FUNC_DEF(test_malloc_zone_ownership_and_destruction),
    FUNC_DEF(test_malloc_zone_names),
    FUNC_DEF(test_posix_memalign_contracts),
    FUNC_DEF(test_malloc_zone_allocation_errors),
    FUNC_DEF(test_fcvt),
    FUNC_DEF(test_tempfile),
    FUNC_DEF(test_NSSet_setWithObjects_count),
};
// clang-format on

int TestApp_cli_tests_main(void) {
#ifdef DEFINE_ME_WHEN_BUILDING_ON_MACOS
  setbuf(stdout, NULL);
#endif

  int tests_run = 0;
  int tests_passed = 0;

  int n = sizeof(test_func_array) / sizeof(test_func_array[0]);
  int i;
  for (i = 0; i < n; i++) {
    printf("%s: ", test_func_array[i].name);
    tests_run++;
    int latest_test_result = test_func_array[i].func();
    if (latest_test_result == 0) {
      printf("OK\n");
      tests_passed++;
    } else {
      printf("FAIL (%d)\n", latest_test_result);
    }
  }

  printf("Passed %d out of %d tests\n", tests_passed, tests_run);
  return tests_run == tests_passed ? 0 : 1;
}
