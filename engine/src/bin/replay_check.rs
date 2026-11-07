//! M4 check: replay battles recorded from Showdown (tools/trace_showdown.js record) in the exact
//! engine with the same seed and the same choices, and report the first difference in the
//! random numbers drawn, the state at each decision, or the battle log.
//!
//! Usage: replay_check <dex.json> <effects.json> <trace.json> [--show N] [--quiet]

use serde_json::{json, Value};
use vgcsim::sim;
use vgcsim::sim::battle::Battle;
use vgcsim::sim::data::BOOST_NAMES;
use vgcsim::sim::state::*;
use vgcsim::sim::val::Mon;

fn snapshot(b: &Battle) -> Value {
    let d = b.d;
    let sides: Vec<Value> = (0..2)
        .map(|s| {
            let side = &b.sides[s];
            let mons: Vec<Value> = (0..side.mons.len())
                .map(|i| {
                    let m = Mon { s: s as u8, i: i as u8 };
                    let p = b.p(m);
                    let mut boosts = serde_json::Map::new();
                    for (k, n) in BOOST_NAMES.iter().enumerate() {
                        boosts.insert(n.to_string(), json!(p.boosts[k]));
                    }
                    let mut vol: Vec<String> = p.volatiles.iter().map(|v| d.eff(v.0).id.clone()).collect();
                    vol.sort();
                    let st = &p.status_state;
                    let status_turns = match st.get(K::Time) {
                        vgcsim::sim::val::Val::Num(x) => json!(x),
                        _ => match st.get(K::Stage) {
                            vgcsim::sim::val::Val::Num(x) => json!(x),
                            _ => Value::Null,
                        },
                    };
                    json!({
                        "species": d.species[p.species as usize].id,
                        "hp": p.hp, "maxhp": p.maxhp,
                        "status": d.eff(p.status).id,
                        "fainted": p.fainted,
                        "boosts": boosts,
                        "item": d.eff(p.item).id,
                        "ability": d.eff(p.ability).id,
                        "active": if p.is_active { p.position as i32 } else { -1 },
                        "vol": vol,
                        "statusTurns": status_turns,
                    })
                })
                .collect();
            let mut sc = serde_json::Map::new();
            for (id, st) in &side.side_conditions {
                let v = st.duration.map(|x| json!(x)).unwrap_or_else(|| match st.get(K::Layers) {
                    vgcsim::sim::val::Val::Num(x) => json!(x),
                    _ => json!(1),
                });
                sc.insert(d.eff(*id).id.clone(), v);
            }
            json!({"mons": mons, "sc": sc})
        })
        .collect();
    let mut pw = serde_json::Map::new();
    for (id, st) in &b.field.pseudo_weather {
        pw.insert(d.eff(*id).id.clone(), st.duration.map(|x| json!(x)).unwrap_or(json!(1)));
    }
    json!({
        "turn": b.turn,
        "sides": sides,
        "weather": d.eff(b.field.weather).id,
        "weatherDur": b.field.weather_state.duration,
        "terrain": d.eff(b.field.terrain).id,
        "terrainDur": b.field.terrain_state.duration,
        "pw": pw,
    })
}

/// differences between two snapshots (ours, Showdown's)
fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in y {
                if k == "seed" {
                    continue;
                }
                let av = x.get(k).unwrap_or(&Value::Null);
                diff(&format!("{path}.{k}"), av, v, out);
            }
            for (k, v) in x {
                if !y.contains_key(k) && !v.is_null() {
                    out.push(format!("{path}.{k}: ours {v}, showdown missing"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: ours {a}, showdown {b}"));
                return;
            }
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                diff(&format!("{path}[{i}]"), p, q, out);
            }
        }
        _ => {
            let eq = match (a.as_f64(), b.as_f64()) {
                (Some(p), Some(q)) => p == q,
                _ => a == b,
            };
            if !eq {
                out.push(format!("{path}: ours {a}, showdown {b}"));
            }
        }
    }
}

/// Showdown's battle.log from |start on, with split lines resolved (secret version) and noise removed
fn norm_showdown_log(log: &[Value]) -> Vec<String> {
    let lines: Vec<&str> = log.iter().filter_map(|x| x.as_str()).collect();
    let mut out = vec![];
    let mut started = false;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with("|split|") {
            if i + 1 < lines.len() {
                let secret = lines[i + 1];
                if started && keep_line(secret) {
                    out.push(secret.to_string());
                }
            }
            i += 3;
            continue;
        }
        if l == "|start" {
            started = true;
        }
        if started && keep_line(l) {
            out.push(l.to_string());
        }
        i += 1;
    }
    out
}
fn keep_line(l: &str) -> bool {
    !(l.starts_with("|t:|") || l.starts_with("|uhtml") || l.is_empty() || l.starts_with("|-hint|"))
}
fn norm_our_log(log: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut started = false;
    for l in log {
        if l == "|start" {
            started = true;
        }
        if started && keep_line(l) {
            out.push(l.clone());
        }
    }
    out
}

fn sd_range(c: &Value) -> (u32, u32) {
    let from = c["from"].as_f64();
    let to = c["to"].as_f64();
    match (from, to) {
        (None, None) => (0, 0),
        (Some(f), None) => (0, f as u32),
        (Some(f), Some(t)) => (f as u32, t as u32),
        (None, Some(t)) => (0, t as u32),
    }
}

fn describe_call(c: &Value, log: &[Value]) -> String {
    let st: Vec<&str> = c["st"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let l = c["L"].as_u64().unwrap_or(0) as usize;
    let last = if l > 0 { log.get(l - 1).and_then(|x| x.as_str()).unwrap_or("") } else { "" };
    format!("range {:?} ctx={} ev={} eff={} stack={} | last log line: {}", sd_range(c), c["top"], c["ev"], c["eff"], st.join("<"), last)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: replay_check <dex.json> <effects.json> <trace.json> [--show N] [--quiet]");
        std::process::exit(2);
    }
    let d = sim::load_data(&args[1], &args[2]).expect("data");
    let recs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[3]).unwrap()).unwrap();
    let show: usize = args.iter().position(|a| a == "--show").map(|p| args[p + 1].parse().unwrap()).unwrap_or(3);
    let quiet = args.iter().any(|a| a == "--quiet");
    let only: Option<u64> = args.iter().position(|a| a == "--only").map(|p| args[p + 1].parse().unwrap());
    let mut ok_battles = 0;
    let mut shown = 0;
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    let mut missing: std::collections::BTreeMap<String, usize> = Default::default();
    for rec in &recs {
        let n = rec["n"].as_u64().unwrap_or(0);
        if let Some(o) = only {
            if n != o {
                continue;
            }
        }
        let seed: Vec<u16> = rec["seed"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect();
        let ta = parse_team(rec["a"].as_str().unwrap());
        let tb = parse_team(rec["b"].as_str().unwrap());
        let names: Vec<String> = rec["names"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_else(|| vec!["A".into(), "B".into()]);
        let mut b = match Battle::new_opts(d, [seed[0], seed[1], seed[2], seed[3]], [&ta, &tb], [&names[0], &names[1]], true, true) {
            Ok(b) => b,
            Err(e) => {
                *reasons.entry(format!("setup error: {e}")).or_default() += 1;
                continue;
            }
        };
        let calls = rec["calls"].as_array().unwrap();
        let sd_log = rec["log"].as_array().unwrap();
        let mut problem: Option<(String, String)> = None;
        let check_rng = |b: &Battle, upto: usize| -> Option<(String, String)> {
            let ours = b.prng.trace.as_ref().unwrap();
            let lim = upto.min(calls.len());
            for k in 0..lim.max(ours.len().min(lim)) {
                let c = &calls[k];
                if k >= ours.len() {
                    return Some(("rng: showdown drew more".into(), format!("call #{k}: showdown {} ; ours: none (we drew {})", describe_call(c, sd_log), ours.len())));
                }
                let (lo, hi, _, _) = ours[k];
                let (slo, shi) = sd_range(c);
                if (lo, hi) != (slo, shi) {
                    return Some((format!("rng: range differs at {}", c["top"]), format!("call #{k}: showdown {} ; ours range ({lo}, {hi})", describe_call(c, sd_log))));
                }
            }
            if ours.len() > upto {
                let (lo, hi, _, _) = ours[upto];
                return Some(("rng: we drew more".into(), format!("call #{upto}: ours range ({lo}, {hi}); showdown drew nothing more here")));
            }
            None
        };
        let steps = rec["steps"].as_array().unwrap();
        for st in steps {
            let upto = st["calls"].as_u64().unwrap() as usize;
            if let Some(p) = check_rng(&b, upto) {
                problem = Some(p);
                break;
            }
            if let Some(snap) = st.get("snap").filter(|s| !s.is_null()) {
                let ours = snapshot(&b);
                let mut dd = vec![];
                diff("", &ours, snap, &mut dd);
                if !dd.is_empty() {
                    problem = Some((format!("state: {}", dd[0].split(':').next().unwrap_or("").trim_start_matches('.')), dd.join("\n    ")));
                    break;
                }
            }
            if args.iter().any(|a| a == "--pp") && only.is_some() {
                let mut line = format!("turn {}:", b.turn);
                for s in 0..2u8 {
                    for i in 0..b.sides[s as usize].mons.len() as u8 {
                        let p = b.p(Mon { s, i });
                        let pps: Vec<String> = p.move_slots.iter().map(|x| format!("{}={}", d.moves[x.id as usize].id, x.pp)).collect();
                        line.push_str(&format!(" | {} {}", p.name, pps.join(",")));
                    }
                }
                println!("{line}");
            }
            let side = st["side"].as_u64().unwrap() as usize;
            let choice = st["choice"].as_str().unwrap();
            let sd_ok = st["ok"].as_bool().unwrap_or(true);
            let ok = b.choose(side, choice);
            if ok != sd_ok {
                problem = Some(("choice validity differs".into(), format!("side {side} choice '{choice}': ours ok={ok} showdown ok={sd_ok} (our error: {})", b.sides[side].choice.error)));
                break;
            }
            // Showdown's run applied 'default' after an invalid choice only when the trace says so
            if !ok && st["applied"].as_str() == Some("default") {
                b.choose(side, "default");
            }
        }
        if problem.is_none() {
            if let Some(p) = check_rng(&b, calls.len()) {
                problem = Some(p);
            }
        }
        if problem.is_none() {
            if let Some(fin) = rec.get("final").filter(|s| !s.is_null()) {
                let ours = snapshot(&b);
                let mut dd = vec![];
                diff("", &ours, fin, &mut dd);
                if !dd.is_empty() {
                    problem = Some(("final state".into(), dd.join("\n    ")));
                }
            }
        }
        // the log
        let ol = norm_our_log(b.log.as_ref().unwrap());
        let sl = norm_showdown_log(sd_log);
        let first_log_diff = (0..ol.len().max(sl.len())).find(|&i| ol.get(i) != sl.get(i));
        if problem.is_none() {
            if let Some(i) = first_log_diff {
                let ctx: Vec<String> = (i.saturating_sub(3)..(i + 1).min(sl.len())).map(|k| format!("      sd  {}", sl[k])).collect();
                problem = Some((
                    format!("log: {}", sl.get(i).map(|l| l.split('|').nth(1).unwrap_or("")).unwrap_or("(end)")),
                    format!("line {i}:\n      ours {}\n      sd   {}\n    showdown context:\n{}", ol.get(i).map(|s| s.as_str()).unwrap_or("(end)"), sl.get(i).map(|s| s.as_str()).unwrap_or("(end)"), ctx.join("\n")),
                ));
            }
        }
        for m in &b.missing {
            *missing.entry(m.clone()).or_default() += 1;
        }
        match problem {
            None => ok_battles += 1,
            Some((reason, detail)) => {
                *reasons.entry(reason.clone()).or_default() += 1;
                if shown < show && !quiet {
                    shown += 1;
                    println!("=== battle {n}: {reason}\n    {detail}");
                    if let Some(i) = first_log_diff {
                        println!("    (first log difference at line {i}: ours {:?} / sd {:?})", ol.get(i), sl.get(i));
                        if args.iter().any(|a| a == "--log") {
                            // both logs side by side around the first difference
                            for k in i.saturating_sub(15)..(i + 6) {
                                println!("    {}{k:4} ours {:<70} | sd {}", if k == i { ">" } else { " " },
                                         ol.get(k).cloned().unwrap_or_default(), sl.get(k).cloned().unwrap_or_default());
                            }
                        }
                    }
                    if !b.missing.is_empty() {
                        println!("    unimplemented handlers reached: {:?}", b.missing);
                    }
                    if only.is_some() {
                        // aligned random-call dump around the first difference
                        let ours = b.prng.trace.as_ref().unwrap();
                        let first = (0..calls.len().max(ours.len())).find(|&k| {
                            let o = ours.get(k).map(|x| (x.0, x.1));
                            let sd = calls.get(k).map(sd_range);
                            o != sd
                        }).unwrap_or(0);
                        for k in first.saturating_sub(6)..(first + 6).min(calls.len().max(ours.len())) {
                            let our_log = b.log.as_ref().unwrap();
                            let o = ours.get(k).map(|x| {
                                let last = if x.3 > 0 { our_log.get(x.3 as usize - 1).cloned().unwrap_or_default() } else { String::new() };
                                format!("{:?} after {}", (x.0, x.1, x.2), last)
                            }).unwrap_or_default();
                            let sd = calls.get(k).map(|c| describe_call(c, sd_log)).unwrap_or_default();
                            println!("    {}#{k}: ours {o}
           sd {sd}", if k == first { ">" } else { " " });
                        }
                        println!("    our log:");
                        for l in ol.iter() {
                            println!("      {l}");
                        }
                    }
                }
            }
        }
    }
    println!("\n{ok_battles}/{} battles identical (rng + state + log)", recs.len());
    let mut rv: Vec<_> = reasons.into_iter().collect();
    rv.sort_by(|a, b| b.1.cmp(&a.1));
    for (r, c) in rv.iter().take(40) {
        println!("  {c:5}  {r}");
    }
    if !missing.is_empty() {
        let mut mv: Vec<_> = missing.into_iter().collect();
        mv.sort_by(|a, b| b.1.cmp(&a.1));
        println!("unimplemented handlers reached (battles):");
        for (m, c) in mv.iter().take(60) {
            println!("  {c:5}  {m}");
        }
    }
}
