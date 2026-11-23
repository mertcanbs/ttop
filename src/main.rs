mod app;
mod model;
mod procs;
mod tmux;
mod ui;

use std::{
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use anyhow::{Result, bail};
use ratatui::crossterm::event::{self, Event};

use crate::{app::Sort, model::Snapshot, procs::Sampler};

const USAGE: &str = "ttop - htop for tmux

usage: ttop [-i SECS] [-s cpu|mem|name]

  -i, --interval SECS   refresh interval (default 2)
  -s, --sort KEY        initial sort: cpu, mem, name (default cpu)
  -h, --help            show this help";

enum Ev {
    Snap(Snapshot),
    Err(String),
    Input(Event),
}

fn main() -> Result<()> {
    let (interval, sort) = args()?;
    let (tx, rx) = mpsc::channel();
    let (refresh, refresh_rx) = mpsc::channel::<()>();

    let ctx = tx.clone();
    thread::spawn(move || {
        let mut s = Sampler::new();
        // first cpu reading needs a baseline; take the second one quickly
        let mut wait = Duration::from_millis(400);
        loop {
            let ev = match collect(&mut s) {
                Ok(snap) => Ev::Snap(snap),
                Err(e) => Ev::Err(e.to_string()),
            };
            if ctx.send(ev).is_err() {
                return;
            }
            if let Err(RecvTimeoutError::Disconnected) = refresh_rx.recv_timeout(wait) {
                return;
            }
            wait = interval;
        }
    });

    // blocking reader so the main loop idles on one channel with no polling
    thread::spawn(move || {
        while let Ok(e) = event::read() {
            if tx.send(Ev::Input(e)).is_err() {
                return;
            }
        }
    });

    let mut app = app::App::new(interval, sort);
    let mut term = ratatui::init();
    let res = (|| -> Result<()> {
        while !app.quit {
            term.draw(|f| ui::draw(f, &mut app))?;
            let mut ev = rx.recv()?;
            loop {
                match ev {
                    Ev::Snap(s) => app.on_snapshot(s),
                    Ev::Err(e) => app.status = Some(e),
                    Ev::Input(Event::Key(k)) => {
                        if app.on_key(k) {
                            let _ = refresh.send(());
                        }
                    }
                    Ev::Input(_) => {}
                }
                // coalesce queued events into one redraw
                match rx.try_recv() {
                    Ok(e) if !app.quit => ev = e,
                    _ => break,
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    res
}

fn collect(s: &mut Sampler) -> Result<Snapshot> {
    let mut sessions = tmux::list()?;
    let sys = s.begin();
    for sess in &mut sessions {
        for w in &mut sess.windows {
            for p in &mut w.panes {
                p.stats = s.tree(p.pid, &mut p.procs);
                w.stats += p.stats;
            }
            sess.stats += w.stats;
        }
    }
    s.end();
    Ok(Snapshot { sys, sessions })
}

fn args() -> Result<(Duration, Sort)> {
    use lexopt::prelude::*;
    let (mut interval, mut sort) = (2.0f64, Sort::Cpu);
    let mut p = lexopt::Parser::from_env();
    while let Some(a) = p.next()? {
        match a {
            Short('i') | Long("interval") => interval = p.value()?.parse()?,
            Short('s') | Long("sort") => {
                sort = match p.value()?.string()?.as_str() {
                    "cpu" => Sort::Cpu,
                    "mem" => Sort::Mem,
                    "name" => Sort::Name,
                    s => bail!("unknown sort: {s}"),
                }
            }
            Short('h') | Long("help") => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(a.unexpected().into()),
        }
    }
    if interval.is_nan() || interval < 0.1 {
        bail!("interval must be >= 0.1");
    }
    Ok((Duration::from_secs_f64(interval), sort))
}
