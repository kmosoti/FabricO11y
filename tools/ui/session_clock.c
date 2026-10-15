#define _GNU_SOURCE
#include <dlfcn.h>
#include <endian.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

struct observation { uint64_t pid, sequence, offset_s, clock_id, raw_ns, returned_ns; };
static _Thread_local struct observation last;
static int (*underlying)(clockid_t, struct timespec *);
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pid_t original;
static int control = -1, ready;
static uint64_t sequence, offset, packets, ledger_bytes;

static void fail(const char *reason) {
    (void)write(STDERR_FILENO, reason, strlen(reason)); (void)write(STDERR_FILENO, "\n", 1); _exit(78);
}
static void receive(int initial) {
    for (unsigned n = 0; n < 65; ++n) {
        uint64_t frame[3]; ssize_t size = recv(control, frame, sizeof frame, MSG_DONTWAIT | MSG_TRUNC);
        if (size < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { if (initial) fail("FCLK-INIT"); return; }
        if (size < 0) fail("FCLK-RECV");
        if (!size) fail("FCLK-EOF");
        if (size != 24 || memcmp(frame, "FCLKv1\0\0", 8)) fail("FCLK-FRAME");
        if (packets >= 64) fail("FCLK-COUNT");
        uint64_t next = le64toh(frame[1]), value = le64toh(frame[2]);
        if (next != packets) fail("FCLK-SEQUENCE");
        if (value > 28860 || value < offset || (!packets && value)) fail("FCLK-OFFSET");
        sequence = next; offset = value; ++packets; if (initial) return;
    }
    fail("FCLK-COUNT");
}
__attribute__((constructor(101))) static void initialize(void) {
    underlying = (int (*)(clockid_t, struct timespec *))dlsym(RTLD_NEXT, "clock_gettime");
    if (!underlying) fail("FCLK-SYMBOL");
    const char *text = getenv("FABRIC_CLOCK_FD"); char *end; long value;
    if (!text || !*text) fail("FCLK-FD");
    errno = 0; value = strtol(text, &end, 10);
    if (errno || *end || value < 0 || value > 1048575) fail("FCLK-FD");
    control = (int)value; int domain, type, flags = fcntl(control, F_GETFD); socklen_t length = sizeof(int);
    if (flags < 0) fail("FCLK-FD");
    if (getsockopt(control, SOL_SOCKET, SO_DOMAIN, &domain, &length) || domain != AF_UNIX
        || getsockopt(control, SOL_SOCKET, SO_TYPE, &type, &length) || type != SOCK_SEQPACKET) fail("FCLK-TYPE");
    int status = fcntl(control, F_GETFL);
    if (status < 0 || !(status & O_NONBLOCK) || fcntl(control, F_SETFD, flags | FD_CLOEXEC)) fail("FCLK-FD");
    original = getpid(); receive(1);
    if (unsetenv("FABRIC_CLOCK_FD") || unsetenv("LD_PRELOAD")) fail("FCLK-ENV");
    ready = 1;
}
struct observation fabric_clock_last(void) { return last; }
int clock_gettime(clockid_t id, struct timespec *result) {
    if (!ready) fail("FCLK-INIT");
    pid_t pid = getpid();
    if (pid != original) {
        int status = underlying(id, result);
        if (!status && id == CLOCK_REALTIME) {
            last.pid = (uint64_t)pid; last.clock_id = (uint64_t)id;
            last.raw_ns = last.returned_ns = (uint64_t)result->tv_sec * 1000000000ULL + (uint64_t)result->tv_nsec;
        }
        return status;
    }
    if (id != CLOCK_REALTIME && id != CLOCK_MONOTONIC && id != CLOCK_MONOTONIC_RAW) return underlying(id, result);
    unsigned attempt;
    for (attempt = 0; attempt < 64 && pthread_mutex_trylock(&mutex); ++attempt) sched_yield();
    if (attempt == 64) fail("FCLK-LOCK");
    if (id == CLOCK_REALTIME) receive(0);
    int status = underlying(id, result), saved_errno = errno;
    if (!status) {
        uint64_t raw = (uint64_t)result->tv_sec * 1000000000ULL + (uint64_t)result->tv_nsec;
        time_t shifted;
        if (id == CLOCK_REALTIME) {
            if (__builtin_add_overflow(result->tv_sec, (time_t)offset, &shifted)) fail("FCLK-OVERFLOW");
            result->tv_sec = shifted;
        }
        last = (struct observation){(uint64_t)pid, sequence, offset, (uint64_t)id, raw,
                                   (uint64_t)result->tv_sec * 1000000000ULL + (uint64_t)result->tv_nsec};
        char line[256]; int size = snprintf(line, sizeof line,
            "{\"pid\":%llu,\"sequence\":%llu,\"offset_s\":%llu,\"clock_id\":%llu,\"raw_ns\":%llu,\"returned_ns\":%llu}",
            (unsigned long long)last.pid, (unsigned long long)last.sequence, (unsigned long long)last.offset_s,
            (unsigned long long)last.clock_id, (unsigned long long)last.raw_ns, (unsigned long long)last.returned_ns);
        if (size < 0 || (size_t)size >= sizeof line || ledger_bytes + (uint64_t)size > 1048576) fail("FCLK-LEDGER");
        if (send(control, line, (size_t)size, MSG_DONTWAIT | MSG_NOSIGNAL) != size) fail("FCLK-LEDGER");
        ledger_bytes += (uint64_t)size;
    }
    if (pthread_mutex_unlock(&mutex)) fail("FCLK-LOCK");
    errno = saved_errno; return status;
}
