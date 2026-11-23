//! macOS process sampling via libproc + Mach host stats.

use std::{collections::HashMap, ffi::c_void, mem, ptr, time::Instant};

use libc::{c_int, mach_msg_type_number_t, mach_port_t, proc_bsdinfo, rusage_info_v2};

use crate::model::{Proc, Stats, Sys};

struct Raw {
    pid: i32,
    ppid: i32,
    name: [u8; 32],
}

pub struct Sampler {
    pids: Vec<i32>,
    /// Sorted by pid.
    raw: Vec<Raw>,
    /// (ppid, raw idx), sorted; children of p are a contiguous range.
    kids: Vec<(i32, u32)>,
    stack: Vec<(u32, u8)>,
    /// pid → cumulative cpu time (mach ticks), previous and current tick.
    prev: HashMap<i32, u64>,
    cur: HashMap<i32, u64>,
    /// cpu ticks delta × scale = percent of one core.
    scale: f64,
    /// ns per mach tick (1 on Intel, 125/3 on Apple Silicon).
    ns_per_tick: f64,
    last: Option<Instant>,
    host: mach_port_t,
    cpu_prev: [u32; 4],
    page: u64,
    mem_total: u64,
}

impl Sampler {
    #[allow(deprecated)]
    pub fn new() -> Self {
        let mut tb = libc::mach_timebase_info { numer: 1, denom: 1 };
        unsafe { libc::mach_timebase_info(&mut tb) };
        let mut mem_total = 0u64;
        let mut sz = size_of::<u64>();
        unsafe {
            libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut mem_total).cast(), &mut sz, ptr::null_mut(), 0)
        };
        Self {
            pids: Vec::new(),
            raw: Vec::new(),
            kids: Vec::new(),
            stack: Vec::new(),
            prev: HashMap::new(),
            cur: HashMap::new(),
            scale: 0.0,
            ns_per_tick: tb.numer as f64 / tb.denom.max(1) as f64,
            last: None,
            host: unsafe { libc::mach_host_self() },
            cpu_prev: [0; 4],
            page: unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64,
            mem_total,
        }
    }

    /// Refreshes the process table and system stats. Call once per tick, before `tree`.
    pub fn begin(&mut self) -> Sys {
        let now = Instant::now();
        self.scale = self.last.map_or(0.0, |t| self.ns_per_tick / now.duration_since(t).as_nanos() as f64 * 100.0);
        self.last = Some(now);

        let n = unsafe { libc::proc_listallpids(ptr::null_mut(), 0) }.max(0) as usize + 64;
        self.pids.clear();
        self.pids.reserve(n);
        let got = unsafe {
            libc::proc_listallpids(self.pids.as_mut_ptr().cast(), (self.pids.capacity() * 4) as c_int)
        };
        unsafe { self.pids.set_len((got.max(0) as usize).min(self.pids.capacity())) };

        self.raw.clear();
        const SZ: c_int = size_of::<proc_bsdinfo>() as c_int;
        let mut bi: proc_bsdinfo = unsafe { mem::zeroed() };
        for &pid in &self.pids {
            if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut bi).cast(), SZ) } != SZ {
                continue;
            }
            let mut name = [0u8; 32];
            // pbi_name is the longer name; empty for some procs, fall back to comm
            let src: &[libc::c_char] = if bi.pbi_name[0] != 0 { &bi.pbi_name } else { &bi.pbi_comm };
            for (d, &s) in name.iter_mut().zip(src) {
                *d = s as u8;
            }
            self.raw.push(Raw { pid, ppid: bi.pbi_ppid as i32, name });
        }
        self.raw.sort_unstable_by_key(|r| r.pid);
        self.kids.clear();
        self.kids.extend(self.raw.iter().enumerate().map(|(i, r)| (r.ppid, i as u32)));
        self.kids.sort_unstable();

        self.sys()
    }

    /// Walks root's process tree, appending to `out` in DFS order; returns the subtree total.
    pub fn tree(&mut self, root: i32, out: &mut Vec<Proc>) -> Stats {
        let Self { raw, kids, stack, prev, cur, scale, .. } = self;
        let mut st = Stats::default();
        let Ok(ri) = raw.binary_search_by_key(&root, |r| r.pid) else { return st };
        stack.clear();
        stack.push((ri as u32, 0));
        while let Some((i, d)) = stack.pop() {
            let r = &raw[i as usize];
            let (ticks, mem) = rusage(r.pid).unwrap_or((0, 0));
            let cpu = match prev.get(&r.pid) {
                Some(&p) if ticks >= p => ((ticks - p) as f64 * *scale) as f32,
                _ => 0.0,
            };
            cur.insert(r.pid, ticks);
            let len = r.name.iter().position(|&b| b == 0).unwrap_or(32);
            out.push(Proc {
                pid: r.pid,
                depth: d,
                name: String::from_utf8_lossy(&r.name[..len]).into_owned(),
                cpu,
                mem,
            });
            st += Stats { cpu, mem, nprocs: 1 };

            let a = kids.partition_point(|&(pp, _)| pp < r.pid);
            let b = a + kids[a..].partition_point(|&(pp, _)| pp == r.pid);
            // reversed so children pop in ascending pid order
            for &(_, c) in kids[a..b].iter().rev() {
                if c != i {
                    stack.push((c, d.saturating_add(1)));
                }
            }
        }
        st
    }

    pub fn end(&mut self) {
        mem::swap(&mut self.prev, &mut self.cur);
        self.cur.clear();
    }

    fn sys(&mut self) -> Sys {
        let mut load: libc::host_cpu_load_info = unsafe { mem::zeroed() };
        let mut cnt: mach_msg_type_number_t = 4; // HOST_CPU_LOAD_INFO_COUNT
        unsafe { libc::host_statistics64(self.host, libc::HOST_CPU_LOAD_INFO, (&raw mut load).cast(), &mut cnt) };
        let t = load.cpu_ticks;
        let d: [u64; 4] = std::array::from_fn(|i| t[i].wrapping_sub(self.cpu_prev[i]) as u64);
        self.cpu_prev = t;
        let busy = d[libc::CPU_STATE_USER as usize] + d[libc::CPU_STATE_SYSTEM as usize] + d[libc::CPU_STATE_NICE as usize];
        let total = busy + d[libc::CPU_STATE_IDLE as usize];

        let mut vm: libc::vm_statistics64 = unsafe { mem::zeroed() };
        let mut cnt = (size_of::<libc::vm_statistics64>() / size_of::<libc::integer_t>()) as mach_msg_type_number_t;
        unsafe { libc::host_statistics64(self.host, libc::HOST_VM_INFO64, (&raw mut vm).cast(), &mut cnt) };
        // Activity Monitor "Memory Used" = app (internal - purgeable) + wired + compressed
        let pages = (vm.internal_page_count as u64).saturating_sub(vm.purgeable_count as u64)
            + vm.wire_count as u64
            + vm.compressor_page_count as u64;

        Sys {
            cpu: if total == 0 { 0.0 } else { (busy as f64 / total as f64 * 100.0) as f32 },
            mem_used: pages * self.page,
            mem_total: self.mem_total,
        }
    }
}

/// (cumulative cpu ticks, phys_footprint bytes)
fn rusage(pid: i32) -> Option<(u64, u64)> {
    let mut ri: rusage_info_v2 = unsafe { mem::zeroed() };
    let r = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&raw mut ri).cast::<*mut c_void>()) };
    (r == 0).then_some((ri.ri_user_time + ri.ri_system_time, ri.ri_phys_footprint))
}
