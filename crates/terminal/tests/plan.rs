//! Plan cases T3 (spawn latency) and T6 (copy from selection) at library level.

use std::time::{Duration, Instant};

use alacritty_terminal::event::Event;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::Processor;
use futures::StreamExt;
use terminal::backend::{new_term, GridSize, Pty, SpawnOptions};

const SIZE: GridSize = GridSize { cols: 80, rows: 24, cell_width: 8.0, cell_height: 16.0 };

/// T3: `/bin/sh` (no user rc) produces its first output in < 50 ms and the
/// spawn call itself is short enough to run off the UI thread.
#[test]
fn t3_first_output_under_50ms() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        let (term, listener, mut events) = new_term(SIZE, 1000);
        let opts = SpawnOptions { shell: Some("/bin/sh".into()), cwd: None, scrollback: 1000 };
        let start = Instant::now();
        let _pty = Pty::spawn(term, listener, &opts, SIZE, 1).expect("spawn");
        let spawn = start.elapsed();
        let first = futures::executor::block_on(async {
            while let Some(ev) = events.next().await {
                if matches!(ev, Event::Wakeup) {
                    return Some(start.elapsed());
                }
            }
            None
        })
        .expect("no output");
        samples.push((spawn, first));
    }
    let median = {
        let mut v: Vec<Duration> = samples.iter().map(|s| s.1).collect();
        v.sort();
        v[v.len() / 2]
    };
    eprintln!("spawn/first-output samples: {samples:?}");
    assert!(median < Duration::from_millis(50), "median first output {median:?}");
}

/// T6: the text of a selection is what gets copied.
#[test]
fn t6_selection_to_string() {
    let (term, _listener, _events) = new_term(SIZE, 1000);
    let mut term = term.lock();
    let mut parser: Processor = Processor::new();
    parser.advance(&mut *term, b"first line\r\nPASTE_2\r\nthird");
    let mut sel = Selection::new(SelectionType::Lines, Point::new(Line(1), Column(2)), Side::Left);
    sel.update(Point::new(Line(1), Column(2)), Side::Right);
    term.selection = Some(sel);
    assert_eq!(term.selection_to_string().unwrap().trim_end(), "PASTE_2");
}
