#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/uio.h>
#include <sys/syscall.h>
static unsigned long matches;
static int injected;
static int partial_fd=-1;
static unsigned long long spill_bytes,spill_calls;
static void measure(int fd,ssize_t n){
    const char *root=getenv("FABRIC_MEASURE_ROOT");if(!root || n<=0)return;
    char link[80],path[PATH_MAX];snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
    ssize_t len=readlink(link,path,sizeof(path)-1);if(len<0)return;path[len]=0;
    if(!strncmp(path,root,strlen(root)) && path[strlen(root)]=='/' && strstr(path,".run-")){
        __atomic_fetch_add(&spill_bytes,(unsigned long long)n,__ATOMIC_RELAXED);
        __atomic_fetch_add(&spill_calls,1,__ATOMIC_RELAXED);
    }
}
__attribute__((destructor)) static void finish(void){
    if(getenv("FABRIC_MEASURE_ROOT")){
        char note[256];int n=snprintf(note,sizeof(note),"FABRIC_SPILL bytes=%llu calls=%llu\n",spill_bytes,spill_calls);
        syscall(SYS_write,2,note,n);
    }
}
static int selected(const char *op, const char *path) {
    const char *root=getenv("FABRIC_FAULT_ROOT"), *pattern=getenv("FABRIC_FAULT_MATCH"), *want=getenv("FABRIC_FAULT_OP");
    if(!root || !pattern || !want || strcmp(want,op) || strncmp(path,root,strlen(root)) || path[strlen(root)]!='/')return 0;
    size_t plen=strlen(pattern), len=strlen(path);
    int matched=plen && pattern[plen-1]=='$' ? len>=plen-1 && !memcmp(path+len-(plen-1),pattern,plen-1) : strstr(path,pattern)!=0;
    if(!matched || injected)return 0;
    const char *at=getenv("FABRIC_FAULT_N");
    if(++matches != (at?strtoul(at,0,10):1))return 0;
    injected=1;
    char note[8192];int n=snprintf(note,sizeof(note),"FABRIC_INJECTION op=%s path=%s match=%lu mode=%s\n",op,path,matches,getenv("FABRIC_FAULT_PARTIAL")?"short-then-EIO":"failure-or-cut");
    syscall(SYS_write,2,note,n);
    if(getenv("FABRIC_FAULT_PAUSE")){raise(SIGSTOP);return 0;}
    if(getenv("FABRIC_FAULT_KILL"))raise(SIGKILL);
    if(getenv("FABRIC_FAULT_PARTIAL"))return 2;
    errno=ENOSPC;return 1;
}
static int fd_selected(const char *op,int fd){
    if(fd==partial_fd){partial_fd=-1;errno=EIO;return 1;}
    char link[80],path[PATH_MAX];snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
    ssize_t n=readlink(link,path,sizeof(path)-1);if(n<0)return 0;path[n]=0;return selected(op,path);
}
ssize_t write(int fd,const void *buf,size_t n){
    static ssize_t(*real)(int,const void*,size_t);if(!real)real=dlsym(RTLD_NEXT,"write");
    int hit=fd_selected("write",fd);if(hit==1)return -1;if(hit==2){partial_fd=fd;n=n<16?n:16;}
    ssize_t result=real(fd,buf,n);measure(fd,result);return result;
}
ssize_t writev(int fd,const struct iovec *v,int n){
    static ssize_t(*real)(int,const struct iovec*,int);if(!real)real=dlsym(RTLD_NEXT,"writev");
    int hit=fd_selected("write",fd);if(hit==1)return -1;
    struct iovec short_v;
    if(hit==2){partial_fd=fd;short_v=v[0];if(short_v.iov_len>16)short_v.iov_len=16;v=&short_v;n=1;}
    ssize_t result=real(fd,v,n);measure(fd,result);return result;
}
ssize_t read(int fd,void *buf,size_t n){
    static ssize_t(*real)(int,void*,size_t);if(!real)real=dlsym(RTLD_NEXT,"read");
    int hit=fd_selected("read",fd);if(hit==1){errno=EIO;return -1;}if(hit==2){partial_fd=fd;n=n<16?n:16;}return real(fd,buf,n);
}
ssize_t pread64(int fd,void *buf,size_t n,off64_t at){
    static ssize_t(*real)(int,void*,size_t,off64_t);if(!real)real=dlsym(RTLD_NEXT,"pread64");
    int hit=fd_selected("read",fd);if(hit==1){errno=EIO;return -1;}if(hit==2){partial_fd=fd;n=n<16?n:16;}return real(fd,buf,n,at);
}
int fsync(int fd){
    static int(*real)(int);if(!real)real=dlsym(RTLD_NEXT,"fsync");
    if(fd_selected("sync",fd))return -1;return real(fd);
}
int fdatasync(int fd){
    static int(*real)(int);if(!real)real=dlsym(RTLD_NEXT,"fdatasync");
    if(fd_selected("sync",fd))return -1;return real(fd);
}
int rename(const char *old,const char *new){
    static int(*real)(const char*,const char*);if(!real)real=dlsym(RTLD_NEXT,"rename");
    if(selected("rename",old))return -1;return real(old,new);
}
