//! Random-vs-random battles on given team pairs, with the same policy as
//! tools/showdown_random_battles.js, to compare statistics with Showdown.
//! Usage: random_battles <dex.json> <pairs.json> <seed> [--log N]

use std::time::Instant;

use serde_json::Value;
use vgcsim::battle::{Battle, Phase};
use vgcsim::dex::Dex;
use vgcsim::state::{parse_team, Rng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dex = Dex::load(&args[1]).expect("dex");
    let pairs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let seed: u64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(1);
    let log_n: usize = args.iter().position(|a| a == "--log").map(|p| args[p + 1].parse().unwrap()).unwrap_or(0);
    let diag_file: Option<String> = args.iter().position(|a| a == "--diag").map(|p| args[p + 1].clone());
    let mut all = vgcsim::battle::Diag::default();

    let mut rng = Rng::new(seed ^ 0xABCDEF);
    let (mut turns, mut p1_wins, mut ties, mut unfinished) = (0u64, 0u32, 0u32, 0u32);
    let mut c = vgcsim::battle::Counters::default();
    let start = Instant::now();
    let mut total_turn_steps = 0u64;
    for (n, p) in pairs.iter().enumerate() {
        let ta = parse_team(p["a"].as_str().unwrap());
        let tb = parse_team(p["b"].as_str().unwrap());
        let mut b = match Battle::new(&dex, [&ta, &tb], seed.wrapping_mul(1000003).wrapping_add(n as u64)) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("battle {n}: {e}");
                unfinished += 1;
                continue;
            }
        };
        if n < log_n {
            b.log = Some(vec![]);
        }
        if diag_file.is_some() {
            b.diag = Some(Box::default());
        }
        b.choose_team(&dex, 0, &[0, 1, 2, 3]);
        b.choose_team(&dex, 1, &[0, 1, 2, 3]);
        let mut guard = 0;
        while b.phase != Phase::Ended && guard < 2000 {
            guard += 1;
            for side in 0..2 {
                if b.needs_choice(side) {
                    let acts = vgcsim::policy::random_actions(&b, &dex, side, &mut rng);
                    b.choose(&dex, side, acts);
                }
            }
            if b.turn > 300 {
                break;
            }
        }
        total_turn_steps += guard as u64;
        match b.winner {
            Some(0) => p1_wins += 1,
            Some(2) => ties += 1,
            Some(_) => {}
            None => unfinished += 1,
        }
        turns += b.turn as u64;
        let bc = b.counters;
        c.moves += bc.moves;
        c.crits += bc.crits;
        c.misses += bc.misses;
        c.protected += bc.protected;
        c.switches += bc.switches;
        c.faints += bc.faints;
        c.cant += bc.cant;
        if let Some(d) = b.diag.take() {
            for (k, v) in d.damage { let e = all.damage.entry(k).or_default(); e.0 += v.0; e.1 += v.1; }
            for (k, v) in d.heal { let e = all.heal.entry(k).or_default(); e.0 += v.0; e.1 += v.1; }
            for (k, v) in d.cant { *all.cant.entry(k).or_default() += v; }
            for (k, v) in d.uses { *all.uses.entry(k).or_default() += v; }
        }
        if let Some(log) = &b.log {
            println!("=== battle {n}: winner {:?}, {} turns\n{}", b.winner, b.turn, log.join("\n"));
        }
    }
    if let Some(f) = &diag_file {
        let j = serde_json::json!({
            "damage": all.damage.iter().map(|(k, v)| (k.clone(), serde_json::json!([v.0, v.1]))).collect::<serde_json::Map<_, _>>(),
            "heal": all.heal.iter().map(|(k, v)| (k.clone(), serde_json::json!([v.0, v.1]))).collect::<serde_json::Map<_, _>>(),
            "cant": all.cant.iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
            "uses": all.uses.iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
        });
        std::fs::write(f, j.to_string()).unwrap();
    }
    let n = pairs.len() as f64;
    let secs = start.elapsed().as_secs_f64();
    let per = |x: u32| x as f64 / n;
    println!(
        "{{\"engine\": \"vgcsim\", \"battles\": {}, \"turns\": {:.3}, \"p1_win\": {:.4}, \"ties\": {}, \"unfinished\": {}, \"moves\": {:.3}, \"crits\": {:.3}, \"misses\": {:.3}, \"protected\": {:.3}, \"switches\": {:.3}, \"faints\": {:.3}, \"cant\": {:.3}, \"seconds\": {:.2}, \"battles_per_s\": {:.0}, \"steps\": {}}}",
        pairs.len(), turns as f64 / n, p1_wins as f64 / n, ties, unfinished,
        per(c.moves), per(c.crits), per(c.misses), per(c.protected), per(c.switches), per(c.faints), per(c.cant),
        secs, n / secs, total_turn_steps
    );
}
