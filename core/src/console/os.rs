use super::protocol::ConsoleLimits;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEvent {
    EvaluatorReadable,
    EvaluatorClosed,
    ClientReadable,
    ClientClosed,
    Timeout,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::mem;
    use std::os::raw::{c_int, c_short, c_uint, c_ulong, c_void};

    const SOL_SOCKET: c_int = 1;
    const SO_PEERCRED: c_int = 17;
    const F_GETFD: c_int = 1;
    const F_SETFD: c_int = 2;
    const FD_CLOEXEC: c_int = 1;
    const RLIMIT_CPU: c_int = 0;
    const RLIMIT_FSIZE: c_int = 1;
    const RLIMIT_CORE: c_int = 4;
    const RLIMIT_NOFILE: c_int = 7;
    const RLIMIT_AS: c_int = 9;
    const PR_SET_DUMPABLE: c_int = 4;
    const PR_SET_NO_NEW_PRIVS: c_int = 38;
    const PR_SET_SECCOMP: c_int = 22;
    const SECCOMP_MODE_FILTER: c_ulong = 2;
    const SIGKILL: c_int = 9;
    const EINTR: c_int = 4;

    const POLLIN: c_short = 0x001;
    const POLLERR: c_short = 0x008;
    const POLLHUP: c_short = 0x010;
    const POLLNVAL: c_short = 0x020;

    const BPF_LD: u16 = 0x00;
    const BPF_W: u16 = 0x00;
    const BPF_ABS: u16 = 0x20;
    const BPF_JMP: u16 = 0x05;
    const BPF_JEQ: u16 = 0x10;
    const BPF_K: u16 = 0x00;
    const BPF_RET: u16 = 0x06;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const EPERM: u32 = 1;

    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xc000_00b7;

    #[repr(C)]
    struct UCred {
        pid: c_int,
        uid: c_uint,
        gid: c_uint,
    }

    #[repr(C)]
    struct RLimit {
        rlim_cur: u64,
        rlim_max: u64,
    }

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: c_short,
        revents: c_short,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *mut SockFilter,
    }

    extern "C" {
        fn geteuid() -> c_uint;
        fn getsockopt(
            socket: c_int,
            level: c_int,
            option_name: c_int,
            option_value: *mut c_void,
            option_len: *mut c_uint,
        ) -> c_int;
        fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
        fn setrlimit(resource: c_int, limit: *const RLimit) -> c_int;
        fn prctl(option: c_int, ...) -> c_int;
        fn poll(fds: *mut PollFd, count: usize, timeout: c_int) -> c_int;
        fn kill(pid: c_int, signal: c_int) -> c_int;
    }

    pub fn effective_uid() -> u32 {
        unsafe { geteuid() }
    }

    pub fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
        let mut credential = UCred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = mem::size_of::<UCred>() as c_uint;
        let status = unsafe {
            getsockopt(
                stream.as_raw_fd(),
                SOL_SOCKET,
                SO_PEERCRED,
                (&mut credential as *mut UCred).cast(),
                &mut length,
            )
        };
        if status != 0 || length as usize != mem::size_of::<UCred>() {
            return Err(format!(
                "cannot authenticate console peer: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(credential.uid)
    }

    pub unsafe fn configure_child(
        inherited_fds: &[RawFd],
        limits: ConsoleLimits,
    ) -> io::Result<()> {
        for fd in inherited_fds {
            let flags = fcntl(*fd, F_GETFD);
            if flags < 0 || fcntl(*fd, F_SETFD, flags & !FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        set_limit(RLIMIT_AS, limits.memory_bytes)?;
        set_limit(
            RLIMIT_CPU,
            limits.evaluation_millis.saturating_add(999) / 1_000 + 1,
        )?;
        set_limit(RLIMIT_FSIZE, 0)?;
        set_limit(RLIMIT_CORE, 0)?;
        set_limit(RLIMIT_NOFILE, 16)?;
        if prctl(PR_SET_DUMPABLE, 0 as c_ulong) != 0 {
            return Err(io::Error::last_os_error());
        }
        if prctl(
            PR_SET_NO_NEW_PRIVS,
            1 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    unsafe fn set_limit(resource: c_int, value: u64) -> io::Result<()> {
        let limit = RLimit {
            rlim_cur: value,
            rlim_max: value,
        };
        if setrlimit(resource, &limit) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn kill_process(pid: u32) {
        let _ = unsafe { kill(pid as c_int, SIGKILL) };
    }

    pub fn wait_event(
        evaluator: RawFd,
        client: RawFd,
        timeout: Duration,
    ) -> Result<WaitEvent, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(WaitEvent::Timeout);
            }
            let timeout_ms = remaining.as_millis().min(c_int::MAX as u128) as c_int;
            let mut fds = [
                PollFd {
                    fd: evaluator,
                    events: POLLIN | POLLHUP | POLLERR,
                    revents: 0,
                },
                PollFd {
                    fd: client,
                    events: POLLIN | POLLHUP | POLLERR,
                    revents: 0,
                },
            ];
            let ready = unsafe { poll(fds.as_mut_ptr(), fds.len(), timeout_ms) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(EINTR) {
                    continue;
                }
                return Err(format!("cannot monitor console evaluation: {error}"));
            }
            if ready == 0 {
                return Ok(WaitEvent::Timeout);
            }
            if fds[1].revents & (POLLHUP | POLLERR | POLLNVAL) != 0 {
                return Ok(WaitEvent::ClientClosed);
            }
            if fds[1].revents & POLLIN != 0 {
                return Ok(WaitEvent::ClientReadable);
            }
            if fds[0].revents & (POLLHUP | POLLERR | POLLNVAL) != 0 {
                return Ok(WaitEvent::EvaluatorClosed);
            }
            if fds[0].revents & POLLIN != 0 {
                return Ok(WaitEvent::EvaluatorReadable);
            }
        }
    }

    pub fn install_evaluator_sandbox() -> Result<(), String> {
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            return Err(
                "console evaluator seccomp is unsupported on this Linux architecture".into(),
            );
        }
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let mut filters = Vec::new();
            filters.push(statement(BPF_LD | BPF_W | BPF_ABS, 4));
            filters.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH, 1, 0));
            filters.push(statement(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));
            filters.push(statement(BPF_LD | BPF_W | BPF_ABS, 0));
            for syscall in denied_syscalls() {
                filters.push(jump(BPF_JMP | BPF_JEQ | BPF_K, syscall, 0, 1));
                filters.push(statement(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));
            }
            filters.push(statement(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
            let mut program = SockFprog {
                len: u16::try_from(filters.len())
                    .map_err(|_| "console seccomp program is too large")?,
                filter: filters.as_mut_ptr(),
            };
            let status = unsafe {
                prctl(
                    PR_SET_SECCOMP,
                    SECCOMP_MODE_FILTER,
                    (&mut program as *mut SockFprog) as c_ulong,
                )
            };
            if status != 0 {
                return Err(format!(
                    "cannot install console evaluator seccomp policy: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    fn statement(code: u16, k: u32) -> SockFilter {
        SockFilter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }

    fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
        SockFilter { code, jt, jf, k }
    }

    #[cfg(target_arch = "x86_64")]
    fn denied_syscalls() -> Vec<u32> {
        vec![
            2, 85, 257, 437, // open, creat, openat, openat2
            41, 53, 42, 49, 50, 43, 288, // sockets
            59, 322, 56, 57, 58, 435, // exec and process creation
            62, 200, 234, // process signalling
            101, 310, 311, // ptrace and cross-process memory
            165, 166, // mounts
            80, 81, 78, 217, 262, 332, // cwd, directory and path inspection
            82, 83, 84, 87, 88, 90, 92, 94, 95, 258, 263, 264, 265, 266, 267, 268, 269,
            316, // path mutation and metadata
            319, 321, 298, 250, // memfd, bpf, perf and keyrings
        ]
    }

    #[cfg(target_arch = "aarch64")]
    fn denied_syscalls() -> Vec<u32> {
        vec![
            56, 437, // openat, openat2
            198, 199, 203, 200, 201, 202, 242, // sockets
            221, 281, 220, 435, // exec and process creation
            129, 130, 131, // process signalling
            117, 270, 271, // ptrace and cross-process memory
            40, 39, // mounts
            49, 50, 61, 79, 291, // cwd, directory and path inspection
            35, 34, 37, 38, 276, 277, 278, // unlink, mkdir, rename and links
            53, 54, 55, 57, 58, 59, 60, 61, // metadata and directory operations
            279, 280, 241, 219, // memfd, bpf, perf and keyrings
        ]
    }
}

#[cfg(target_os = "linux")]
pub use linux::{
    configure_child, effective_uid, install_evaluator_sandbox, kill_process, peer_uid, wait_event,
};

#[cfg(not(target_os = "linux"))]
pub fn effective_uid() -> u32 {
    u32::MAX
}

#[cfg(not(target_os = "linux"))]
pub fn peer_uid(_stream: &UnixStream) -> Result<u32, String> {
    Err("the separate-process console currently requires Linux peer credentials".into())
}

#[cfg(not(target_os = "linux"))]
pub unsafe fn configure_child(_inherited_fds: &[RawFd], _limits: ConsoleLimits) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the separate-process console currently requires Linux rlimits",
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn kill_process(_pid: u32) {}

#[cfg(not(target_os = "linux"))]
pub fn wait_event(
    _evaluator: RawFd,
    _client: RawFd,
    _timeout: Duration,
) -> Result<WaitEvent, String> {
    Err("the separate-process console currently requires Linux poll".into())
}

#[cfg(not(target_os = "linux"))]
pub fn install_evaluator_sandbox() -> Result<(), String> {
    Err("the separate-process console currently requires Linux seccomp".into())
}

pub fn authenticate_peer(stream: &UnixStream) -> Result<(), String> {
    let peer = peer_uid(stream)?;
    let owner = effective_uid();
    if peer != owner {
        return Err(format!(
            "console peer uid {peer} does not match supervisor uid {owner}"
        ));
    }
    Ok(())
}

pub fn raw_fd(stream: &UnixStream) -> RawFd {
    stream.as_raw_fd()
}
