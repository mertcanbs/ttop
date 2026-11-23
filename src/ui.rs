use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
};

use crate::{
    app::{App, HIST, Hist, Mode},
    model::Key,
};

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn draw(f: &mut Frame, app: &mut App) {
    let [head, bars, _, body, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(f.area());

    title(f, head, app);

    let [cpu, mem] = Layout::horizontal([Constraint::Fill(1); 2]).spacing(3).areas(bars);
    let sys = &app.snap.sys;
    f.render_widget(bar("CPU", sys.cpu as f64 / 100.0, &format!("{:.1}%", sys.cpu), cpu.width), cpu);
    let mf = sys.mem_used as f64 / sys.mem_total.max(1) as f64;
    let mt = format!("{}/{}", fmt_mem(sys.mem_used), fmt_mem(sys.mem_total));
    f.render_widget(bar("MEM", mf, &mt, mem.width), mem);

    table(f, body, app);
    footer(f, foot, app);
}

fn title(f: &mut Frame, area: Rect, app: &App) {
    let ss = &app.snap.sessions;
    let nw: usize = ss.iter().map(|s| s.windows.len()).sum();
    let np: usize = ss.iter().flat_map(|s| &s.windows).map(|w| w.panes.len()).sum();
    let left = Line::from(vec![
        " ttop ".black().on_cyan().bold(),
        format!("  {} sessions · {nw} windows · {np} panes", ss.len()).into(),
    ]);
    let mut right = vec![];
    if !app.filter.is_empty() {
        right.push("filter: ".dark_gray());
        right.push(app.filter.clone().yellow());
        right.push("  ".into());
    }
    right.push("sort: ".dark_gray());
    right.push(app.sort.name().into());
    right.push(format!("  {}s ", app.interval.as_secs_f32()).dark_gray());
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(Line::from(right).right_aligned()), area);
}

/// htop-style meter: `CPU [|||||||        34.0%]`
fn bar(label: &'static str, frac: f64, text: &str, w: u16) -> Line<'static> {
    let room = (w as usize).saturating_sub(label.len() + 3 + text.len());
    let fill = ((frac.clamp(0.0, 1.0) * room as f64).round() as usize).min(room);
    let color = match frac {
        x if x >= 0.85 => Color::Red,
        x if x >= 0.6 => Color::Yellow,
        _ => Color::Green,
    };
    Line::from(vec![
        label.cyan().bold(),
        " [".into(),
        Span::styled("|".repeat(fill), Style::new().fg(color)),
        " ".repeat(room - fill).into(),
        Span::styled(text.to_owned(), Style::new().dark_gray()),
        "]".into(),
    ])
}

fn table(f: &mut Frame, area: Rect, app: &mut App) {
    let total = app.snap.sys.mem_total.max(1) as f64;
    let rows = app.rows.iter().map(|r| {
        let mut name = Vec::with_capacity(5);
        name.push(Span::raw("  ".repeat(r.depth as usize)));
        name.push(match r.open {
            Some(true) => "▾ ".dark_gray(),
            Some(false) => "▸ ".dark_gray(),
            None => "  ".into(),
        });
        let mut ls = match r.key {
            Key::Session(_) => Style::new().bold(),
            Key::Window(_) => Style::new(),
            Key::Pane(_) => Style::new(),
            Key::Proc(_) => Style::new().fg(Color::Gray),
        };
        if r.here {
            ls = ls.fg(Color::Cyan);
        }
        name.push(Span::styled(r.label.as_str(), ls));
        if !r.note.is_empty() {
            name.push(" ".into());
            name.push(Span::styled(r.note.as_str(), Style::new().dark_gray()));
        }

        let s = r.stats;
        let procs = if matches!(r.key, Key::Proc(_)) { String::new() } else { s.nprocs.to_string() };
        let cpu_style = match s.cpu {
            x if x >= 80.0 => Style::new().red().bold(),
            x if x >= 30.0 => Style::new().yellow(),
            x if x < 0.05 => Style::new().dark_gray(),
            _ => Style::new(),
        };
        Row::new([
            Cell::from(Line::from(name)),
            Cell::from(Line::from(procs).right_aligned().dark_gray()),
            Cell::from(Line::styled(format!("{:.1}", s.cpu), cpu_style).right_aligned()),
            Cell::from(Line::from(fmt_mem(s.mem)).right_aligned()),
            Cell::from(Line::from(format!("{:.1}", s.mem as f64 / total * 100.0)).right_aligned().dark_gray()),
            Cell::from(Line::from(spark(app.hist.get(&r.key))).green()),
        ])
    });

    let header = Row::new([
        Cell::from("NAME"),
        Cell::from(Line::from("PROCS").right_aligned()),
        Cell::from(Line::from("CPU%").right_aligned()),
        Cell::from(Line::from("MEM").right_aligned()),
        Cell::from(Line::from("MEM%").right_aligned()),
        Cell::from("CPU HISTORY"),
    ])
    .style(Style::new().black().on_green().bold());

    let t = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(HIST.max(11) as u16),
        ],
    )
    .header(header)
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(t, area, &mut app.table);
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let line = match &app.mode {
        Mode::Filter => Line::from(vec!["/".yellow().bold(), app.filter.as_str().into(), "█".dark_gray()]),
        Mode::Confirm(_, what) => Line::from(vec![what.as_str().red().bold(), " [y/N]".dark_gray()]),
        Mode::Normal => match &app.status {
            Some(s) => Line::from(s.as_str().red()),
            None => {
                let keys = [
                    ("j/k", "move"),
                    ("␣", "fold"),
                    ("1-4", "depth"),
                    ("g", "goto"),
                    ("s", "sort"),
                    ("/", "filter"),
                    ("x", "kill"),
                    ("q", "quit"),
                ];
                let mut v = Vec::with_capacity(keys.len() * 2);
                for (k, d) in keys {
                    v.push(k.cyan().bold());
                    v.push(format!(" {d}  ").dark_gray());
                }
                Line::from(v)
            }
        },
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Last HIST cpu samples; scaled to max(100%, peak) so multi-core spikes stay visible.
fn spark(h: Option<&Hist>) -> String {
    let mut s = String::with_capacity(HIST * 3);
    let Some(h) = h else { return s };
    let max = h.buf.iter().copied().fold(100f32, f32::max);
    for (i, &v) in h.buf.iter().enumerate() {
        s.push(if i < HIST - h.len { ' ' } else { SPARK[((v / max) * 7.0).round() as usize] });
    }
    s
}

fn fmt_mem(b: u64) -> String {
    const K: f64 = 1024.0;
    let b = b as f64;
    if b >= K * K * K {
        format!("{:.1}G", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.0}M", b / (K * K))
    } else {
        format!("{:.0}K", b / K)
    }
}
