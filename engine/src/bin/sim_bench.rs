//! Speed of the exact engine: random-vs-random battles on given team pairs.
//! Usage: sim_bench <dex.json> <effects.json> <pairs.json> [count]

use std::time::Instant;

use serde_json::Value;
use vgcsim::sim;
use vgcsim::sim::battle::Battle;
use vgcsim::sim::policy::XorShift;
use vgcsim::sim::state::parse_team;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let d = sim::load_data(&args[1], &args[2]).expect("data");
    let pairs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[3]).unwrap()).unwrap();
    let count: usize = args.get(4).map(|x| x.parse().unwrap()).unwrap_or(pairs.len());
    let teams: Vec<_> = pairs.iter().map(|p| (parse_team(p["a"].as_str().unwrap()), parse_team(p["b"].as_str().unwrap()))).collect();
    let start = Instant::now();
    let (mut turns, mut decisions, mut p1) = (0u64, 0u64, 0u32);
    for n in 0..count {
        let (ta, tb) = &teams[n % teams.len()];
        let seed = [(n * 7919 + 13) as u16, (n * 104729 + 7) as u16, (n * 31 + 4242) as u16, (n * 523 + 99) as u16];
        let mut b = Battle::new(d, seed, [ta, tb], ["A", "B"]).unwrap();
        let mut rng = XorShift::new((n as u32).wrapping_mul(2654435761).wrapping_add(12345));
        let mut guard = 0;
        while !b.ended && guard < 2000 && b.turn <= 300 {
            guard += 1;
            let mut any = false;
            for s in 0..2 {
                if let Some(ch) = b.random_choice(s, &mut rng) {
                    if b.is_choice_done_pub(s) {
                        continue;
                    }
                    any = true;
                    decisions += 1;
                    if !b.choose(s, &ch) {
                        b.choose(s, "default");
                    }
                }
            }
            if !any {
                break;
            }
        }
        turns += b.turn as u64;
        if b.winner == Some(Some(0)) {
            p1 += 1;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!("{count} battles in {secs:.2}s = {:.0} battles/s, {:.1} turns/battle, {:.1} decisions/battle, p1 won {:.3}",
        count as f64 / secs, turns as f64 / count as f64, decisions as f64 / count as f64, p1 as f64 / count as f64);
}
