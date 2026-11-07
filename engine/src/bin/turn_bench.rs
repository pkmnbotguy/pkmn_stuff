//! Speed of the search primitives on the exact engine: clone, one turn, payoff matrices.
//! turn_bench data/dex.json data/effects.json ../../Teams_real [positions]
use std::time::Instant;
use vgcsim::sim::{self, battle::Battle, search::SlotAction, state::parse_team};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = sim::load_data(&a[1], &a[2]).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&a[3]).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    let n: usize = a.get(4).map(|x| x.parse().unwrap()).unwrap_or(30);
    let four = |p: &std::path::Path| {
        let t = parse_team(&std::fs::read_to_string(p).unwrap());
        t.into_iter().take(4).collect::<Vec<_>>()
    };
    let ours: Vec<[SlotAction; 2]> = (0..4).flat_map(|i| (0..4).map(move |j| [SlotAction::Move { slot: i, target: 1, mega: false }, SlotAction::Move { slot: j, target: 2, mega: false }])).collect();
    let theirs: Vec<[SlotAction; 2]> = (0..3).flat_map(|i| (0..3).map(move |j| [SlotAction::Move { slot: i, target: 1, mega: false }, SlotAction::Move { slot: j, target: 2, mega: false }])).collect();
    let mut bs = vec![];
    for k in 0..n {
        let (x, y) = (four(&files[2 * k]), four(&files[2 * k + 1]));
        let mut b = Battle::from_brought(d, [&x, &y], [1, 2, 3, k as u16], 3).unwrap();
        b.refresh();
        bs.push(b);
    }
    let reps = 2000;
    let t = Instant::now();
    let mut sink = 0;
    for b in &bs {
        for _ in 0..reps {
            let c = b.clone();
            sink += c.turn;
        }
    }
    let clone_us = t.elapsed().as_secs_f64() / (bs.len() * reps) as f64 * 1e6;
    let t = Instant::now();
    let mut cells = 0;
    for _ in 0..3 {
        for b in &bs {
            let m = b.payoff_matrix(&ours, &theirs, 4, 7, 0.5, &[], 1.0);
            cells += m.len() * m[0].len() * 4;
        }
    }
    let turn_us = t.elapsed().as_secs_f64() / cells as f64 * 1e6;
    println!("clone {clone_us:.2} us, clone+turn {turn_us:.1} us ({sink})");
}
