/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
// Modern host observation only, not a historical iOS conformance test.
#include <errno.h>
#include <fcntl.h>
#include <semaphore.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
  char name[32];
  snprintf(name, sizeof(name), "/uf_probe_%d", getpid());
  sem_t *a = sem_open(name, O_CREAT | O_EXCL, 0600, 1);
  if (a == SEM_FAILED)
    return 1;
  sem_t *b = sem_open(name, 0);
  if (b == SEM_FAILED) {
    sem_close(a);
    sem_unlink(name);
    return 2;
  }
  printf("same_handle=%d\n", a == b);
  int result = sem_close(a);
  printf("first_close=%d\n", result);
  errno = 0;
  result = sem_post(b);
  printf("post_after_close=%d errno=%d\n", result, errno);
  errno = 0;
  result = sem_close(b);
  printf("second_close=%d errno=%d\n", result, errno);
  sem_unlink(name);
  return 0;
}
