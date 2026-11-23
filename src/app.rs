use std::{
    collections::{HashMap, HashSet},
    mem,
    time::Duration,
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    widgets::TableState,
};

use crate::{
    model::{Key, Pane, Snapshot, Stats, Window},
    tmux,
};

pub const HIST: usize = 20;

pub struct Hist {
    pub buf: [f32; HIST],
    pub len: usize,
    seen: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Cpu,
    Mem,
    Name,
}

impl Sort {
    pub fn name(self) -> &'static str {
        match self {
            Sort::Cpu => "cpu",
            Sort::Mem => "mem",
            Sort::Name => "name",
        }
    }
}

pub enum Mode {
    Normal,
    Filter,
    Confirm(Key, String),
}

pub struct Row {
    pub key: Key,
    pub depth: u8,
    /// None = leaf.
    pub open: Option<bool>,
    pub label: String,
    pub note: String,
    pub stats: Stats,
    /// On the path to ttop's own pane.
    pub here: bool,
}

pub struct App {
    pub snap: Snapshot,
    pub rows: Vec<Row>,
    pub table: TableState,
    pub hist: HashMap<Key, Hist>,
    pub sort: Sort,
    pub filter: String,
    pub mode: Mode,
    pub status: Option<String>,
    pub interval: Duration,
    pub quit: bool,
    /// Nodes whose open state differs from the `level` default.
    toggled: HashSet<Key>,
    /// Number of tree levels shown by default (1 = sessions only).
    level: u8,
    sel: Option<Key>,
    tick: u32,
    self_pane: Option<u32>,
}

impl App {
    pub fn new(interval: Duration, sort: Sort) -> Self {
        Self {
            snap: Snapshot::default(),
            rows: Vec::new(),
            table: TableState::default(),
            hist: HashMap::new(),
            sort,
            filter: String::new(),
            mode: Mode::Normal,
            status: None,
            interval,
            quit: false,
            toggled: HashSet::new(),
            level: 2,
            sel: None,
            tick: 0,
            self_pane: std::env::var("TMUX_PANE").ok().map(|p| tmux::id(&p)),
        }
    }

    pub fn on_snapshot(&mut self, snap: Snapshot) {
        self.snap = snap;
        self.tick = self.tick.wrapping_add(1);
        let t = self.tick;
        let mut rec = |k: Key, v: f32| {
            let h = self.hist.entry(k).or_insert(Hist { buf: [0.0; HIST], len: 0, seen: t });
            h.buf.copy_within(1.., 0);
            h.buf[HIST - 1] = v;
            h.len = (h.len + 1).min(HIST);
            h.seen = t;
        };
        for s in &self.snap.sessions {
            rec(Key::Session(s.id), s.stats.cpu);
            for w in &s.windows {
                rec(Key::Window(w.id), w.stats.cpu);
                for p in &w.panes {
                    rec(Key::Pane(p.id), p.stats.cpu);
                    for x in &p.procs {
                        rec(Key::Proc(x.pid), x.cpu);
                    }
                }
            }
        }
        self.hist.retain(|_, h| h.seen == t);
        self.sort_snap();
        self.rebuild();
    }

    fn sort_snap(&mut self) {
        let by = |a: &Stats, b: &Stats| match self.sort {
            Sort::Cpu => b.cpu.total_cmp(&a.cpu),
            Sort::Mem => b.mem.cmp(&a.mem),
            Sort::Name => std::cmp::Ordering::Equal,
        };
        let ss = &mut self.snap.sessions;
        ss.sort_by(|a, b| by(&a.stats, &b.stats).then_with(|| a.name.cmp(&b.name)));
        for s in ss {
            s.windows.sort_by(|a, b| by(&a.stats, &b.stats).then(a.index.cmp(&b.index)));
            for w in &mut s.windows {
                w.panes.sort_by(|a, b| by(&a.stats, &b.stats).then(a.index.cmp(&b.index)));
            }
        }
    }

    fn open(&self, k: Key, depth: u8) -> bool {
        (depth + 1 < self.level) != self.toggled.contains(&k)
    }

    /// Flattens the visible tree into `rows`. With a filter, a node is shown if it or a
    /// descendant matches; non-matching ancestors are forced open, matches expand normally.
    fn rebuild(&mut self) {
        let f = self.filter.as_bytes();
        let filt = !f.is_empty();
        let fm = |t: &str| contains_ci(t, f);
        let pm = |p: &Pane| fm(&p.cmd) || p.procs.iter().any(|x| fm(&x.name));
        let wm = |w: &Window| fm(&w.name) || w.panes.iter().any(pm);
        let me = self.self_pane;
        let mut rows = mem::take(&mut self.rows);
        rows.clear();

        for s in &self.snap.sessions {
            let hit = !filt || fm(&s.name);
            if !hit && !s.windows.iter().any(wm) {
                continue;
            }
            let k = Key::Session(s.id);
            let open = !hit || self.open(k, 0);
            let here = me.is_some_and(|m| s.windows.iter().any(|w| w.panes.iter().any(|p| p.id == m)));
            let note = if s.attached { "attached" } else { "" };
            rows.push(Row { key: k, depth: 0, open: Some(open), label: s.name.clone(), note: note.into(), stats: s.stats, here });
            if !open {
                continue;
            }
            for w in &s.windows {
                let hit = hit || fm(&w.name);
                if !hit && !w.panes.iter().any(pm) {
                    continue;
                }
                let k = Key::Window(w.id);
                let open = !hit || self.open(k, 1);
                let here = me.is_some_and(|m| w.panes.iter().any(|p| p.id == m));
                let label = format!("{}:{}", w.index, w.name);
                rows.push(Row { key: k, depth: 1, open: Some(open), label, note: String::new(), stats: w.stats, here });
                if !open {
                    continue;
                }
                for p in &w.panes {
                    let hit = hit || fm(&p.cmd);
                    if !hit && !p.procs.iter().any(|x| fm(&x.name)) {
                        continue;
                    }
                    let k = Key::Pane(p.id);
                    let open = !hit || self.open(k, 2);
                    let label = format!("{}.{} {}", w.index, p.index, p.cmd);
                    let here = me == Some(p.id);
                    rows.push(Row { key: k, depth: 2, open: Some(open), label, note: String::new(), stats: p.stats, here });
                    if !open {
                        continue;
                    }
                    for x in &p.procs {
                        rows.push(Row {
                            key: Key::Proc(x.pid),
                            depth: 3 + x.depth,
                            open: None,
                            label: x.name.clone(),
                            note: x.pid.to_string(),
                            stats: Stats { cpu: x.cpu, mem: x.mem, nprocs: 1 },
                            here: false,
                        });
                    }
                }
            }
        }

        // once the user picked a node, follow it across re-sorts; else keep the index
        let idx = self
            .sel
            .and_then(|k| rows.iter().position(|r| r.key == k))
            .unwrap_or_else(|| self.table.selected().unwrap_or(0));
        self.rows = rows;
        let follow = self.sel.is_some();
        self.select(idx);
        if !follow {
            self.sel = None;
        }
    }

    fn select(&mut self, i: usize) {
        if self.rows.is_empty() {
            self.table.select(None);
            self.sel = None;
            return;
        }
        let i = i.min(self.rows.len() - 1);
        self.table.select(Some(i));
        self.sel = Some(self.rows[i].key);
    }

    fn toggle(&mut self, i: usize) {
        let Some(r) = self.rows.get(i) else { return };
        if r.open.is_some() {
            let k = r.key;
            if !self.toggled.remove(&k) {
                self.toggled.insert(k);
            }
            self.rebuild();
        }
    }

    fn parent(&self, i: usize, pred: impl Fn(&Row) -> bool) -> Option<usize> {
        let d = self.rows.get(i)?.depth;
        self.rows[..i].iter().rposition(|r| r.depth < d && pred(r))
    }

    /// Returns true when the collector should refresh immediately.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if k.kind != KeyEventKind::Press {
            return false;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return false;
        }
        match self.mode {
            Mode::Normal => return self.normal(k),
            Mode::Filter => {
                match k.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char(c) => self.filter.push(c),
                    _ => return false,
                }
                self.sel = None;
                self.table.select(Some(0));
                self.rebuild();
            }
            Mode::Confirm(key, _) => {
                self.mode = Mode::Normal;
                if matches!(k.code, KeyCode::Char('y' | 'Y')) {
                    let r = match key {
                        Key::Proc(pid) => {
                            if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                                Ok(())
                            } else {
                                Err(anyhow::anyhow!("kill {pid}: {}", std::io::Error::last_os_error()))
                            }
                        }
                        _ => tmux::kill(key),
                    };
                    self.status = r.err().map(|e| e.to_string());
                    return true;
                }
            }
        }
        false
    }

    fn normal(&mut self, k: KeyEvent) -> bool {
        use KeyCode::*;
        self.status = None;
        let i = self.table.selected().unwrap_or(0);
        match k.code {
            Char('q') => self.quit = true,
            Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild();
            }
            Char('j') | Down => self.select(i + 1),
            Char('k') | Up => self.select(i.saturating_sub(1)),
            PageDown => self.select(i + 20),
            PageUp => self.select(i.saturating_sub(20)),
            Home => self.select(0),
            End => self.select(usize::MAX),
            Char(' ') | Enter => self.toggle(i),
            Char('l') | Right => match self.rows.get(i).and_then(|r| r.open) {
                Some(false) => self.toggle(i),
                Some(true) => self.select(i + 1),
                None => {}
            },
            Char('h') | Left => match self.rows.get(i).and_then(|r| r.open) {
                Some(true) => self.toggle(i),
                _ => {
                    if let Some(p) = self.parent(i, |_| true) {
                        self.select(p);
                    }
                }
            },
            Char(c @ '1'..='4') => {
                self.level = c as u8 - b'0';
                self.toggled.clear();
                self.rebuild();
            }
            Char('s') => {
                self.sort = match self.sort {
                    Sort::Cpu => Sort::Mem,
                    Sort::Mem => Sort::Name,
                    Sort::Name => Sort::Cpu,
                };
                self.sort_snap();
                self.rebuild();
            }
            Char('/') => self.mode = Mode::Filter,
            Char('g') => {
                let Some(r) = self.rows.get(i) else { return false };
                let key = match r.key {
                    Key::Proc(_) => match self.parent(i, |r| matches!(r.key, Key::Pane(_))) {
                        Some(p) => self.rows[p].key,
                        None => return false,
                    },
                    k => k,
                };
                self.status = tmux::switch(key).err().map(|e| e.to_string());
            }
            Char('x') => {
                if let Some(r) = self.rows.get(i) {
                    let kind = match r.key {
                        Key::Session(_) => "session",
                        Key::Window(_) => "window",
                        Key::Pane(_) => "pane",
                        Key::Proc(_) => "process",
                    };
                    let what = if r.note.is_empty() || !matches!(r.key, Key::Proc(_)) {
                        format!("kill {kind} {}?", r.label)
                    } else {
                        format!("kill {kind} {} ({})?", r.label, r.note)
                    };
                    self.mode = Mode::Confirm(r.key, what);
                }
            }
            Char('r') => return true,
            _ => {}
        }
        false
    }
}

fn contains_ci(hay: &str, needle: &[u8]) -> bool {
    needle.is_empty() || hay.as_bytes().windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}
