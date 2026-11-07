//! Compare vgcsim's stats and damage rolls with Showdown's, case by case.
//! Usage: damage_check <dex.json> <damage_cases.json> [show N mismatches]

use std::collections::BTreeMap;

use serde_json::Value;
use vgcsim::damage::{damage_full, damage_rolls, Combatant, Conditions, Status, Terrain, Weather};
use vgcsim::dex::{to_id, Dex, STAT_IDS};
use vgcsim::effects::{Ab, It};
use vgcsim::stats::champions_stats;

fn combatant(dex: &Dex, p: &Value) -> Result<(Combatant, Vec<String>), String> {
    let mut problems = Vec::new();
    let species = dex.species(p["species"].as_str().unwrap()).ok_or("unknown species")?;
    let mut points = [0u16; 6];
    for (i, s) in STAT_IDS.iter().enumerate() {
        points[i] = p["set"]["evs"][s].as_u64().unwrap_or(0) as u16;
    }
    let nature = dex.nature(p["set"]["nature"].as_str().unwrap_or(""));
    let stats = champions_stats(&species.base, &points, nature);
    // Showdown keeps the base form's HP after Mega Evolution
    let maxhp = p["maxhp"].as_u64().unwrap() as u16;
    let base_form = dex.species(p["baseSpecies"].as_str().unwrap()).unwrap_or(species);
    let base_hp = champions_stats(&base_form.base, &points, nature)[0];
    if base_hp != maxhp {
        problems.push(format!("maxhp {base_hp} vs showdown {maxhp}"));
    }
    for (i, s) in STAT_IDS.iter().enumerate().skip(1) {
        let sd = p["stats"][s].as_u64().unwrap() as u16;
        if stats[i] != sd {
            problems.push(format!("{s} {} vs showdown {sd}", stats[i]));
        }
    }
    let types_sd: Vec<String> = p["types"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect();
    let mine: Vec<String> = species.types.iter().filter(|&&t| t != 255).map(|&t| dex.type_names[t as usize].clone()).collect();
    if types_sd != mine {
        problems.push(format!("types {mine:?} vs showdown {types_sd:?}"));
    }
    let item_id = p["item"].as_str().unwrap_or("");
    let item = It::from_id(item_id);
    let has_item = !item_id.is_empty();
    let removable = has_item
        && !dex.item(item_id).map(|it| {
            it.mega.iter().any(|(base, _)| {
                dex.species(base).map(|b| b.base_species == species.base_species).unwrap_or(false)
            })
        }).unwrap_or(false);
    let mut boosts = [0i8; 6];
    for (i, s) in STAT_IDS.iter().enumerate().skip(1) {
        boosts[i] = p["boosts"][s].as_i64().unwrap_or(0) as i8;
    }
    let status = match p["status"].as_str().unwrap_or("") {
        "brn" => Status::Burn,
        "par" => Status::Paralysis,
        "psn" => Status::Poison,
        "tox" => Status::Toxic,
        "slp" => Status::Sleep,
        "frz" => Status::Freeze,
        _ => Status::None,
    };
    let mut full = stats;
    full[0] = maxhp;
    Ok((
        Combatant {
            added_type: vgcsim::dex::NO_TYPE,
            berry_weaken: false,
            metro: 0,
            semi: 0,
            types: species.types,
            stats: full,
            hp: p["hp"].as_u64().unwrap() as u16,
            maxhp,
            ability: Ab::from_id(p["ability"].as_str().unwrap_or("")),
            item,
            item_removable: removable,
            has_item,
            boosts,
            status,
            weight_hg: species.weight_hg,
            is_pikachu: species.base_species == "pikachu",
            flash_fire: false,
            disguised: species.id == "mimikyu" && p["ability"].as_str() == Some("disguise"),
            glaive_rush: false,
            last_move_failed: false,
            hurt_this_turn: false,
            charged: false,
            times_attacked: 0,
            forced_grounded: false,
            airborne: false,
            stats_lowered: p["statsLowered"].as_bool().unwrap_or(false),
            retal: [0; 3],
            level: 50,
            gender: 0,
        },
        problems,
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dex = Dex::load(&args[1]).expect("dex");
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let show: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(10);

    let (mut ok, mut wrong, mut stat_wrong) = (0, 0, 0);
    let mut by_key: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut shown = 0;
    let mut prepared = Vec::with_capacity(cases.len());
    for c in &cases {
        // an Imposter Ditto transformed in the oracle's battle: its stats are copied, not computable from its set (skip)
        if c["attacker"]["baseSpecies"].as_str() == Some("ditto") || c["defender"]["baseSpecies"].as_str() == Some("ditto") {
            continue;
        }
        let (att, p1) = combatant(&dex, &c["attacker"]).unwrap();
        let (mut def, mut p2) = combatant(&dex, &c["defender"]).unwrap();
        // Mimicry: the type follows the terrain (set by the battle engine); take the type Showdown's Pokemon had
        if def.ability == Ab::Mimicry {
            let t: Vec<&str> = c["defender"]["types"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
            def.types = [dex.type_id(t[0]), t.get(1).map_or(255, |n| dex.type_id(n))];
            p2.retain(|x| !x.starts_with("types"));
        }
        // the battle engine ignores breakable defender abilities for Mold Breaker & co. (battle.rs hit path): same here
        if matches!(att.ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)
            && !matches!(def.ability, Ab::ShadowShield | Ab::PrismArmor | Ab::Fullmetalbody | Ab::Multitype | Ab::Rkssystem | Ab::Comatose)
        {
            def.ability = Ab::None;
        }
        if !p1.is_empty() || !p2.is_empty() {
            stat_wrong += 1;
            if stat_wrong <= 5 {
                println!("STATS {} / {}: {:?} {:?}", c["attacker"]["species"], c["defender"]["species"], p1, p2);
            }
        }
        let abilities: Vec<&str> = ["attacker", "defender"].iter().map(|k| c[*k]["ability"].as_str().unwrap_or(""))
            .chain([c["attackerAlly"].as_str().unwrap_or(""), c["defenderAlly"].as_str().unwrap_or("")])
            .collect();
        let screens: Vec<&str> = c["screens"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        let cond = Conditions {
            weather: match c["weather"].as_str().unwrap_or("") {
                "raindance" => Weather::Rain,
                "sunnyday" => Weather::Sun,
                "sandstorm" => Weather::Sand,
                "snowscape" => Weather::Snow,
                _ => Weather::None,
            },
            terrain: match c["terrain"].as_str().unwrap_or("") {
                "electricterrain" => Terrain::Electric,
                "grassyterrain" => Terrain::Grassy,
                "psychicterrain" => Terrain::Psychic,
                "mistyterrain" => Terrain::Misty,
                _ => Terrain::None,
            },
            spread: c["spread"].as_bool().unwrap(),
            helping_hand: c["helpingHand"].as_bool().unwrap(),
            reflect: screens.contains(&"reflect"),
            light_screen: screens.contains(&"lightscreen"),
            aurora_veil: screens.contains(&"auroraveil"),
            friend_guard: c["defenderAlly"].as_str() == Some("friendguard"),
            wonder_room: false,
            stakeout: false,
            fairy_aura: abilities.contains(&"fairyaura"),
            dark_aura: abilities.contains(&"darkaura"),
            steely_spirit: c["attackerAlly"].as_str() == Some("steelyspirit"),
            plus_minus: matches!(c["attacker"]["ability"].as_str(), Some("plus") | Some("minus")) && matches!(c["attackerAlly"].as_str(), Some("plus") | Some("minus")),
            berries_blocked: [c["attacker"]["ability"].as_str(), c["attackerAlly"].as_str()].iter()
                .any(|a| matches!(a, Some("unnerve") | Some("asoneglastrier") | Some("asonespectrier"))),
            fainted_on_attacker_side: 0,
            supreme_fallen: 0,
            protect_bypass: false,
            fickle: false,
            // battle-state values the oracle recorded (newer case files): the engine computes them from its own state
            analytic: c["analytic"].as_bool().unwrap_or(false),
            minimized: false,
            hangry: false,
            fling_bp: c["flingBp"].as_u64().unwrap_or(0) as u16,
            acc_eva_boosts: 0,
            bp_override: c["bpCallback"].as_u64().unwrap_or(0) as u16,
            species_move_type: (c["move"].as_str() == Some("ragingbull")).then(|| dex.type_id(c["moveType"].as_str().unwrap())),
            beat_up_bp: 0,
            round_boost: false,
            gem: false,
            parental_second: false,
        };
        let mv = dex.move_by_id(c["move"].as_str().unwrap()).unwrap();
        let crit = c["crit"].as_bool().unwrap_or(false) && !vgcsim::damage::crit_blocked(&def);
        let mine = damage_full(&dex, mv, &att, &def, &cond, crit, 1).map(|h| h.rolls);
        prepared.push((mv, att.clone(), def.clone(), cond));
        // negative rolls = Showdown's raw formula on a move that fails in battle (Endeavor with more HP than the target)
        let negative = c["rolls"].as_array().map_or(false, |r| r.iter().any(|x| x.as_i64().map_or(false, |v| v < 0)))
            // Endeavor with at least as much HP as the target: 0 from the raw formula, the move fails in battle
            || (c["move"].as_str() == Some("endeavor") && c["rolls"].as_array().map_or(false, |r| r.iter().all(|x| x.as_i64() == Some(0))));
        let expected: Option<Vec<u16>> = if c["immune"].as_bool().unwrap() || negative {
            None
        } else {
            Some(c["rolls"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect())
        };
        let mine_v = mine.map(|r| r.to_vec());
        let good = mine_v == expected;
        if good {
            ok += 1;
        } else {
            wrong += 1;
            if shown < show {
                shown += 1;
                println!(
                    "MISMATCH {} ({}, {}) -> {} ({}, {}) move {} [{}] spread={} weather={} terrain={} hh={} screens={:?}\n   mine:     {:?}\n   showdown: {:?}",
                    c["attacker"]["species"], c["attacker"]["ability"], c["attacker"]["item"],
                    c["defender"]["species"], c["defender"]["ability"], c["defender"]["item"],
                    c["move"], c["moveType"], c["spread"], c["weather"], c["terrain"], c["helpingHand"], screens,
                    mine_v, expected
                );
            }
        }
        for key in [
            format!("move:{}", c["move"].as_str().unwrap()),
            format!("att-ability:{}", c["attacker"]["ability"].as_str().unwrap()),
            format!("att-item:{}", c["attacker"]["item"].as_str().unwrap()),
            format!("def-ability:{}", c["defender"]["ability"].as_str().unwrap()),
            format!("def-item:{}", c["defender"]["item"].as_str().unwrap()),
        ] {
            let e = by_key.entry(key).or_insert((0, 0));
            e.0 += 1;
            if !good {
                e.1 += 1;
            }
        }
    }
    println!("\n{} cases: {} exact, {} wrong ({:.2}% exact); {} skipped (Imposter Ditto); {} cases with stat/type differences",
        cases.len(), ok, wrong, 100.0 * ok as f64 / (ok + wrong).max(1) as f64, cases.len() - ok - wrong, stat_wrong);
    let mut worst: Vec<_> = by_key.into_iter().filter(|(_, (_, w))| *w > 0).collect();
    worst.sort_by(|a, b| (b.1 .1 as f64 / b.1 .0 as f64).partial_cmp(&(a.1 .1 as f64 / a.1 .0 as f64)).unwrap().then(b.1 .1.cmp(&a.1 .1)));
    println!("Where the mismatches are (wrong / cases):");
    for (k, (n, w)) in worst.iter().take(25) {
        println!("  {k:<40} {w:>4} / {n}");
    }
    // speed: the damage calculation alone, many passes over the prepared cases
    let reps = 20;
    let start = std::time::Instant::now();
    let mut sink = 0u64;
    for _ in 0..reps {
        for (mv, att, def, cond) in &prepared {
            if let Some(r) = damage_rolls(&dex, mv, att, def, cond) {
                sink += r[0] as u64;
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let calls = (reps * prepared.len()) as f64;
    println!("
Speed: {:.0} damage calculations/s on one thread ({:.2} us each, all 16 rolls) [{}]",
        calls / secs, secs / calls * 1e6, sink % 7);
    let _ = to_id;
}
