#define _GNU_SOURCE
#include <dlfcn.h>
#include <endian.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

struct observation { uint64_t pid, sequence, offset_s, clock_id, raw_ns, returned_ns; };
static _Thread_local struct observation last;
struct timing { uint64_t tid, call_index, monotonic_before_ns, monotonic_after_ns; };
static _Thread_local struct timing timing;
static int (*underlying)(clockid_t, struct timespec *);
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pid_t original;
static int control = -1, ready;
static uint64_t sequence, offset, packets, ledger_bytes, call_index;

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
struct timing fabric_clock_timing(void) { return timing; }
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
    int incoming_errno = errno;
    struct timespec lock_deadline;
    if (underlying(CLOCK_MONOTONIC, &lock_deadline)) fail("FCLK-MONOTONIC");
    lock_deadline.tv_nsec += 100000000L;
    if (lock_deadline.tv_nsec >= 1000000000L) {
        lock_deadline.tv_nsec -= 1000000000L;
        if (__builtin_add_overflow(lock_deadline.tv_sec, (time_t)1, &lock_deadline.tv_sec)) fail("FCLK-LOCK");
    }
    if (pthread_mutex_clocklock(&mutex, CLOCK_MONOTONIC, &lock_deadline)) fail("FCLK-LOCK");
    errno = incoming_errno;
    if (id == CLOCK_REALTIME) receive(0);
    struct timespec before, after;
    if (underlying(CLOCK_MONOTONIC, &before)) fail("FCLK-MONOTONIC");
    int status = underlying(id, result), saved_errno = errno;
    if (underlying(CLOCK_MONOTONIC, &after)) fail("FCLK-MONOTONIC");
    if (!status) {
        uint64_t raw = (uint64_t)result->tv_sec * 1000000000ULL + (uint64_t)result->tv_nsec;
        time_t shifted;
        if (id == CLOCK_REALTIME) {
            if (__builtin_add_overflow(result->tv_sec, (time_t)offset, &shifted)) fail("FCLK-OVERFLOW");
            result->tv_sec = shifted;
        }
        last = (struct observation){(uint64_t)pid, sequence, offset, (uint64_t)id, raw,
                                   (uint64_t)result->tv_sec * 1000000000ULL + (uint64_t)result->tv_nsec};
        timing = (struct timing){(uint64_t)syscall(SYS_gettid), ++call_index,
            (uint64_t)before.tv_sec * 1000000000ULL + (uint64_t)before.tv_nsec,
            (uint64_t)after.tv_sec * 1000000000ULL + (uint64_t)after.tv_nsec};
        char line[256]; int size = snprintf(line, sizeof line,
            "{\"pid\":%llu,\"sequence\":%llu,\"offset_s\":%llu,\"clock_id\":%llu,\"raw_ns\":%llu,\"returned_ns\":%llu,\"tid\":%llu,\"call_index\":%llu,\"monotonic_before_ns\":%llu,\"monotonic_after_ns\":%llu}",
            (unsigned long long)last.pid, (unsigned long long)last.sequence, (unsigned long long)last.offset_s,
            (unsigned long long)last.clock_id, (unsigned long long)last.raw_ns, (unsigned long long)last.returned_ns,
            (unsigned long long)timing.tid, (unsigned long long)timing.call_index,
            (unsigned long long)timing.monotonic_before_ns, (unsigned long long)timing.monotonic_after_ns);
        if (size < 0 || (size_t)size >= sizeof line || ledger_bytes + (uint64_t)size > 1048576) fail("FCLK-LEDGER");
        if (send(control, line, (size_t)size, MSG_DONTWAIT | MSG_NOSIGNAL) != size) fail("FCLK-LEDGER");
        ledger_bytes += (uint64_t)size;
    }
    if (pthread_mutex_unlock(&mutex)) fail("FCLK-LOCK");
    errno = saved_errno; return status;
}
