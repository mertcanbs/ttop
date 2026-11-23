use std::ops::AddAssign;

/// Stable tmux ids ($N, @N, %N) or pid; survives renames/reorders.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Session(u32),
    Window(u32),
    Pane(u32),
    Proc(i32),
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    /// Percent of one core; can exceed 100.
    pub cpu: f32,
    /// phys_footprint bytes.
    pub mem: u64,
    pub nprocs: u32,
}

impl AddAssign for Stats {
    fn add_assign(&mut self, o: Self) {
        self.cpu += o.cpu;
        self.mem += o.mem;
        self.nprocs += o.nprocs;
    }
}

pub struct Proc {
    pub pid: i32,
    /// Depth within the pane's process tree (root shell = 0).
    pub depth: u8,
    pub name: String,
    pub cpu: f32,
    pub mem: u64,
}

pub struct Pane {
    pub id: u32,
    pub index: u32,
    pub pid: i32,
    pub cmd: String,
    pub stats: Stats,
    /// DFS order.
    pub procs: Vec<Proc>,
}

pub struct Window {
    pub id: u32,
    pub index: u32,
    pub name: String,
    pub stats: Stats,
    pub panes: Vec<Pane>,
}

pub struct Session {
    pub id: u32,
    pub name: String,
    pub attached: bool,
    pub stats: Stats,
    pub windows: Vec<Window>,
}

#[derive(Default)]
pub struct Sys {
    /// 0..=100 across all cores.
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
}

#[derive(Default)]
pub struct Snapshot {
    pub sys: Sys,
    pub sessions: Vec<Session>,
}
