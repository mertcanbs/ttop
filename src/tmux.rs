use std::process::Command;

use anyhow::{Result, bail};

use crate::model::{Key, Pane, Session, Stats, Window};

const FMT: &str = "#{session_id}\t#{session_name}\t#{session_attached}\t#{window_id}\t#{window_index}\t#{window_name}\t#{pane_id}\t#{pane_index}\t#{pane_pid}\t#{pane_current_command}";

/// One exec per tick; list-panes -a is ordered session → window → pane.
pub fn list() -> Result<Vec<Session>> {
    let out = run(Command::new("tmux").args(["list-panes", "-a", "-F", FMT]))?;
    let mut sessions: Vec<Session> = Vec::new();
    for line in out.lines() {
        let mut f = line.split('\t');
        let mut next = || f.next().unwrap_or("");
        let (sid, sname, attached) = (id(next()), next(), next() != "0");
        let (wid, widx, wname) = (id(next()), next().parse().unwrap_or(0), next());
        let (pid_, pidx, ppid, cmd) = (id(next()), next().parse().unwrap_or(0), next().parse().unwrap_or(0), next());

        if sessions.last().is_none_or(|s| s.id != sid) {
            sessions.push(Session { id: sid, name: sname.into(), attached, stats: Stats::default(), windows: Vec::new() });
        }
        let s = sessions.last_mut().unwrap();
        if s.windows.last().is_none_or(|w| w.id != wid) {
            s.windows.push(Window { id: wid, index: widx, name: wname.into(), stats: Stats::default(), panes: Vec::new() });
        }
        s.windows.last_mut().unwrap().panes.push(Pane {
            id: pid_,
            index: pidx,
            pid: ppid,
            cmd: cmd.into(),
            stats: Stats::default(),
            procs: Vec::new(),
        });
    }
    Ok(sessions)
}

pub fn switch(k: Key) -> Result<()> {
    // outside tmux, switch-client would hijack whichever client was most recently active
    if std::env::var_os("TMUX").is_none() {
        bail!("goto needs ttop running inside tmux");
    }
    let t = target(k)?;
    let mut c = Command::new("tmux");
    c.args(["switch-client", "-t", &t]);
    if matches!(k, Key::Window(_) | Key::Pane(_)) {
        c.args([";", "select-window", "-t", &t]);
    }
    if matches!(k, Key::Pane(_)) {
        c.args([";", "select-pane", "-t", &t]);
    }
    run(&mut c).map(drop)
}

pub fn kill(k: Key) -> Result<()> {
    let cmd = match k {
        Key::Session(_) => "kill-session",
        Key::Window(_) => "kill-window",
        Key::Pane(_) => "kill-pane",
        Key::Proc(_) => bail!("not a tmux target"),
    };
    run(Command::new("tmux").args([cmd, "-t", &target(k)?])).map(drop)
}

fn target(k: Key) -> Result<String> {
    Ok(match k {
        Key::Session(n) => format!("${n}"),
        Key::Window(n) => format!("@{n}"),
        Key::Pane(n) => format!("%{n}"),
        Key::Proc(_) => bail!("not a tmux target"),
    })
}

fn run(c: &mut Command) -> Result<String> {
    let out = c.output()?;
    if !out.status.success() {
        bail!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout).unwrap_or_default())
}

/// "$12" / "@3" / "%7" → numeric part.
pub fn id(s: &str) -> u32 {
    s.get(1..).and_then(|x| x.parse().ok()).unwrap_or(0)
}
