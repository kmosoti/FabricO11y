#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/openat2.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Isolated syscall demonstration using caller-created synthetic files only.
   This is not a proposed FabricO11y implementation. */
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    const int flags = O_RDONLY | O_NONBLOCK | O_CLOEXEC;
    int fd;
    if (strcmp(argv[1], "all_components") == 0) {
        struct open_how how = {0};
        how.flags = flags | O_NOFOLLOW;
        how.resolve = RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS;
        fd = (int)syscall(SYS_openat2, AT_FDCWD, argv[2], &how, sizeof(how));
    } else if (strcmp(argv[1], "leaf_only") == 0) {
        fd = open(argv[2], flags | O_NOFOLLOW);
    } else if (strcmp(argv[1], "legacy") == 0) {
        fd = open(argv[2], flags);
    } else return 2;
    if (fd < 0) {
        printf("{\"opened\":false,\"errno\":%d}\n", errno);
        return 0;
    }
    struct stat st;
    if (fstat(fd, &st) != 0) { close(fd); return 3; }
    printf("{\"opened\":true,\"regular\":%s}\n", S_ISREG(st.st_mode) ? "true" : "false");
    close(fd);
    return 0;
}
