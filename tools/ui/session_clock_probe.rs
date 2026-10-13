use std::{env, fs, io::{self, BufRead, Write}, os::unix::process::CommandExt, process::Command, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))] compile_error!("clock probe requires Linux x86_64");
#[repr(C)] #[derive(Default)] struct Timespec { sec: i64, nsec: i64 }
#[repr(C)] #[derive(Clone, Copy, Default)] struct Observation { pid: u64, sequence: u64, offset_s: u64, clock_id: u64, raw_ns: u64, returned_ns: u64 }
#[link(name = "dl")] extern "C" {
    fn clock_gettime(id: i32, value: *mut Timespec) -> i32; fn syscall(number: i64, ...) -> i64;
    fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn fcntl(fd: i32, command: i32, ...) -> i32; fn getsockopt(fd: i32, level: i32, name: i32, value: *mut i32, size: *mut u32) -> i32;
    fn fork() -> i32; fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32; fn close(fd: i32) -> i32;
}
fn raw(id: i32) -> u64 { let mut value = Timespec::default(); assert_eq!(unsafe { syscall(228, id, &mut value as *mut Timespec) }, 0); ns(&value) }
fn ns(value: &Timespec) -> u64 { value.sec as u64 * 1_000_000_000 + value.nsec as u64 }
fn env_clear() -> bool { env::var_os("LD_PRELOAD").is_none() && env::var_os("FABRIC_CLOCK_FD").is_none() }
fn row(kind: &str, value: Observation, before: u64, after: u64) {
    println!("{{\"kind\":\"{}\",\"row\":{{\"pid\":{},\"sequence\":{},\"offset_s\":{},\"clock_id\":{},\"raw_ns\":{},\"returned_ns\":{},\"syscall_before_ns\":{},\"syscall_after_ns\":{}}}}}",
        kind, value.pid, value.sequence, value.offset_s, value.clock_id, value.raw_ns, value.returned_ns, before, after);
}
fn main() {
    let args: Vec<_> = env::args().collect(); let fd: i32 = args[2].parse().unwrap(); let helper = &args[3]; let socket = &args[4];
    if args[1] == "ordinary" {
        let before = raw(0); let clock = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(); let after = raw(0);
        let mapped = fs::read_to_string("/proc/self/maps").unwrap().lines().any(|line| line.ends_with(helper));
        let present = fs::read_link(format!("/proc/self/fd/{fd}")).is_ok_and(|path| path.to_string_lossy() == *socket);
        println!("{{\"kind\":\"exec\",\"pid\":{},\"env_clear\":{},\"helper_mapped\":{},\"control_fd_present\":{},\"syscall_before_ns\":{},\"clock_ns\":{},\"syscall_after_ns\":{}}}",
            std::process::id(), env_clear(), mapped, present, before, clock, after); return;
    }
    let symbol = unsafe { dlsym(std::ptr::null_mut(), c"fabric_clock_last".as_ptr()) };
    if symbol.is_null() {
        println!("{{\"kind\":\"absent\",\"pid\":{}}}", std::process::id()); io::stdout().flush().unwrap();
        let mut input = String::new(); io::stdin().read_line(&mut input).unwrap(); eprintln!("FCLK-MISSING"); std::process::exit(76);
    }
    let last: unsafe extern "C" fn() -> Observation = unsafe { std::mem::transmute(symbol) };
    let mut socket_type = 0; let mut length = 4;
    assert_eq!(unsafe { getsockopt(fd, 1, 3, &mut socket_type, &mut length) }, 0);
    println!("{{\"kind\":\"ready\",\"pid\":{},\"env_clear\":{},\"cloexec\":{},\"socket_type\":{}}}",
        std::process::id(), env_clear(), unsafe { fcntl(fd, 1) } & 1 != 0, socket_type); io::stdout().flush().unwrap();
    for input in io::stdin().lock().lines() {
        match input.unwrap().as_str() {
            "clock" => {
                for id in [0, 1, 4] { let before = raw(id); let mut value = Timespec::default(); assert_eq!(unsafe { clock_gettime(id, &mut value) }, 0);
                    let observation = unsafe { last() }; let after = raw(id); assert_eq!(ns(&value), observation.returned_ns); row("clock", observation, before, after); }
                let before = raw(0); let value = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(); let observation = unsafe { last() }; let after = raw(0);
                assert_eq!(value, observation.returned_ns as u128); row("system", observation, before, after);
            }
            "sleep" => { let a = raw(1); let began = Instant::now(); let b = raw(1); std::thread::sleep(Duration::from_millis(500));
                let c = raw(1); let elapsed = began.elapsed().as_nanos(); let d = raw(1);
                println!("{{\"kind\":\"sleep\",\"row\":{{\"start_before_ns\":{a},\"start_after_ns\":{b},\"end_before_ns\":{c},\"end_after_ns\":{d},\"instant_elapsed_ns\":{elapsed}}}}}"); }
            "fork" => { let previous = unsafe { last() }; let child = unsafe { fork() }; assert!(child >= 0);
                if child == 0 { let before = raw(0); let mut value = Timespec::default(); assert_eq!(unsafe { clock_gettime(0, &mut value) }, 0);
                    let observation = unsafe { last() }; let after = raw(0); row("fork", observation, before, after); io::stdout().flush().unwrap();
                    let error = Command::new(env::current_exe().unwrap()).args(["ordinary", &args[2], helper, socket]).exec(); panic!("exec: {error}"); }
                let mut status = 0; assert_eq!(unsafe { waitpid(child, &mut status, 0) }, child);
                println!("{{\"kind\":\"fork_done\",\"pid\":{},\"fork_pid\":{},\"sequence_before\":{},\"status\":{}}}", std::process::id(), child, previous.sequence, status); }
            "close" => { assert_eq!(unsafe { close(fd) }, 0); let _ = SystemTime::now(); }
            "quit" => break,
            _ => panic!("unknown native probe command"),
        }
        io::stdout().flush().unwrap();
    }
}
