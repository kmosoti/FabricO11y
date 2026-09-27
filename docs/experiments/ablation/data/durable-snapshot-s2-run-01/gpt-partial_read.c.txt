#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static int target_fd = -1;
ssize_t read(int fd, void *buf, size_t count) {
    static ssize_t (*real_read)(int, void *, size_t);
    if (!real_read) real_read = dlsym(RTLD_NEXT, "read");
    if (getenv("INJECT_PARTIAL_RAW")) {
        if (fd == target_fd) {
            errno = EIO;
            fprintf(stderr, "injected EIO after 64 raw bytes\n");
            target_fd = -1;
            return -1;
        }
        char fd_path[64], path[4096];
        snprintf(fd_path, sizeof(fd_path), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(fd_path, path, sizeof(path)-1);
        if (n > 0) {
            path[n] = '\0';
            size_t len = strlen(path);
            const char *suffix = "/block-0.json";
            size_t slen = strlen(suffix);
            if (len >= slen && strcmp(path + len - slen, suffix) == 0) {
                target_fd = fd;
                ssize_t got = real_read(fd, buf, count < 64 ? count : 64);
                fprintf(stderr, "injected short raw read: %zd bytes\n", got);
                return got;
            }
        }
    }
    return real_read(fd, buf, count);
}
