//! Throughput of the fast engine: full random-vs-random battles per second on real team pairs
//! (team preview, the random policy, to the end), on one thread and on all threads.
//! Also times a bot-sized one-turn payoff matrix.
//!
//!     fast_bench data/dex.json data/pairs6k.json [battles_per_thread] [threads]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// counts heap allocations (to find allocation hot spots)
struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

use serde_json::Value;
use vgcsim::battle::{build_team, Action, Battle, Phase};
use vgcsim::dex::Dex;
use vgcsim::policy;
use vgcsim::state::{parse_team, Mon, Rng};

static mut T_SETUP: f64 = 0.0;
static mut T_POLICY: f64 = 0.0;
static mut T_CHOOSE: f64 = 0.0;

fn play_timed(dex: &Dex, teams: &[(Vec<Mon>, Vec<Mon>)], n: usize, seed: u64, timed: bool) -> (u64, u64) {
    let mut rng = Rng::new(seed);
    let mut turns = 0u64;
    let now = || if timed { Some(Instant::now()) } else { None };
    let add = |t: Option<Instant>, slot: *mut f64| if let Some(t) = t { unsafe { *slot += t.elapsed().as_secs_f64() } };
    for k in 0..n {
        let (ta, tb) = &teams[(k + seed as usize * 7919) % teams.len()];
        let t0 = now();
        let mut b = Battle::from_mons([ta, tb], seed.wrapping_mul(1000003).wrapping_add(k as u64));
        b.choose_team(dex, 0, &[0, 1, 2, 3]);
        b.choose_team(dex, 1, &[0, 1, 2, 3]);
        add(t0, &raw mut T_SETUP);
        let mut guard = 0;
        while b.phase != Phase::Ended && guard < 2000 && b.turn <= 300 {
            guard += 1;
            for side in 0..2 {
                if b.needs_choice(side) {
                    let t1 = now();
                    let acts = policy::random_actions(&b, dex, side, &mut rng);
                    add(t1, &raw mut T_POLICY);
                    let t2 = now();
                    b.choose(dex, side, acts);
                    add(t2, &raw mut T_CHOOSE);
                }
            }
        }
        turns += b.turn as u64;
    }
    (n as u64, turns)
}

fn play(dex: &Dex, teams: &[(Vec<Mon>, Vec<Mon>)], n: usize, seed: u64) -> (u64, u64) {
    play_timed(dex, teams, n, seed, false)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dex = Dex::load(&args[1]).expect("dex");
    let pairs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let per_thread: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(20000);
    let threads: usize = args.get(4).map(|s| s.parse().unwrap()).unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    let teams: Vec<(Vec<Mon>, Vec<Mon>)> = pairs.iter().map(|p| (build_team(&dex, &parse_team(p["a"].as_str().unwrap())).unwrap(), build_team(&dex, &parse_team(p["b"].as_str().unwrap())).unwrap())).collect();

    // one thread
    let t = Instant::now();
    let (n, turns) = play(&dex, &teams, per_thread, 1);
    let s1 = t.elapsed().as_secs_f64();
    println!("1 thread : {:>8.0} battles/s  ({:.1} turns/battle, {:.2} us/turn)", n as f64 / s1, turns as f64 / n as f64, s1 * 1e6 / turns as f64);

    // allocations per turn
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let (_, t2) = play(&dex, &teams, 2000, 5);
    println!("  heap allocations: {:.1} per turn", (ALLOCS.load(Ordering::Relaxed) - a0) as f64 / t2 as f64);

    // where the single-thread time goes
    play_timed(&dex, &teams, per_thread / 2, 3, true);
    unsafe {
        let tot = T_SETUP + T_POLICY + T_CHOOSE;
        println!("  breakdown: setup {:.0}%  random policy {:.0}%  turn resolution {:.0}%", 100.0 * T_SETUP / tot, 100.0 * T_POLICY / tot, 100.0 * T_CHOOSE / tot);
    }

    // all threads
    let t = Instant::now();
    let total: (u64, u64) = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads).map(|k| {
            let (dex, teams) = (&dex, &teams);
            sc.spawn(move || play(dex, teams, per_thread, 100 + k as u64))
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap()).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    });
    let sa = t.elapsed().as_secs_f64();
    println!("{threads} threads: {:>8.0} battles/s", total.0 as f64 / sa);

    // a bot-sized payoff matrix (16 x 16 joint actions, 4 samples) from turn-2 positions
    let mut rng = Rng::new(7);
    let mut cells = 0u64;
    let mut secs = 0.0;
    for k in 0..200 {
        let (ta, tb) = &teams[k % teams.len()];
        let mut b = Battle::from_mons([ta, tb], k as u64);
        b.choose_team(&dex, 0, &[0, 1, 2, 3]);
        b.choose_team(&dex, 1, &[0, 1, 2, 3]);
        if b.phase == Phase::Ended {
            continue;
        }
        let la = policy::legal_actions(&b, &dex, 0);
        let lb = policy::legal_actions(&b, &dex, 1);
        let pick = |l: &[policy::SlotActions; 2], r: &mut Rng| -> Vec<[Action; 2]> {
            (0..16).map(|_| {
                let a = if l[0].is_empty() { Action::Pass } else { l[0][r.below(l[0].len() as u32) as usize] };
                let b = if l[1].is_empty() { Action::Pass } else { l[1][r.below(l[1].len() as u32) as usize] };
                [a, b]
            }).collect()
        };
        let ours = pick(&la, &mut rng);
        let theirs = pick(&lb, &mut rng);
        let t = Instant::now();
        let m = policy::payoff_matrix(&b, &dex, &ours, &theirs, 4, 11, 0.5, &[], 1.0);
        secs += t.elapsed().as_secs_f64();
        cells += (m.len() * m[0].len() * 4) as u64;
    }
    println!("payoff matrix: {:.2} us per simulated turn ({} turns)", secs * 1e6 / cells as f64, cells);
    // (2026-10-18) the state the search copies once per simulated turn
    {
        let (ta, tb) = &teams[0];
        let mut b = Battle::from_mons([ta, tb], 1);
        b.choose_team(&dex, 0, &[0, 1, 2, 3]);
        b.choose_team(&dex, 1, &[0, 1, 2, 3]);
        let n = 2_000_000;
        let t = Instant::now();
        let mut acc = 0u64;
        for _ in 0..n {
            let c = std::hint::black_box(&b).clone();
            acc = acc.wrapping_add(std::hint::black_box(&c).turn as u64);
        }
        println!("Battle: {} bytes (Mon {} bytes); clone {:.1} ns ({acc})", std::mem::size_of::<Battle>(),
                 std::mem::size_of::<Mon>(), t.elapsed().as_secs_f64() * 1e9 / n as f64);
    }
}
