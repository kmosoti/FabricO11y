#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <unistd.h>

int fsync(int fd) {
    static int calls = 0;
    static int (*real_fsync)(int) = NULL;
    if (!real_fsync) real_fsync = dlsym(RTLD_NEXT, "fsync");
    ++calls;
    const char *value = getenv("FAIL_FSYNC_AT");
    if (value && calls == atoi(value)) {
        errno = EIO;
        return -1;
    }
    return real_fsync(fd);
}
