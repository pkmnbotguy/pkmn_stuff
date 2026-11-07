//! Move handlers (data/moves.ts with the Champions mod) for the moves in the team pool.
//!
//! Argument mapping (singleEvent calls in battle-actions.ts):
//!   onTry(source, target), onTryMove(source, target), onModifyMove/Type(move, source, target),
//!   basePowerCallback / damageCallback(source, target): a.target = the user, a.source = the target.
//!   onHit / onTryHit / onAfterHit / onMoveFail / onPrepareHit / onTryImmunity(target, source):
//!   a.target = the move's target, a.source = the user.

#![allow(unused_imports)]
use super::{amv, holder, src, tgt};
use crate::add;
use crate::sim::battle::*;
use crate::sim::data::*;
use crate::sim::events::Ev;
use crate::sim::state::*;
use crate::sim::val::{Mon, Val};

type H = fn(&mut Battle, &HArgs) -> Val;

pub fn lookup(id: &str, cb: &str) -> Option<HandlerFn> {
    let f: H = match (id, cb) {
        ("instruct", "onHit") => instruct_hit,
        ("curse", "onModifyMove") => curse_modify_move,
        ("curse", "onTryHit") => curse_try_hit,
        ("curse", "onHit") => curse_hit,
        ("transform", "onHit") => transform_hit,
        ("fling", "onPrepareHit") => fling_prepare,
        ("fling", "onHit") => fling_hit,
        ("eeriespell", "secondary.onHit") => eeriespell_secondary,
        ("thief", "onAfterHit") => thief_after_hit,
        ("bounce", "onTryMove") => bounce_try_move,
        ("supercellslam", "onMoveFail") => supercellslam_fail,
        ("noretreat", "onTry") => noretreat_try,
        ("sleeptalk", "onTry") => sleeptalk_try,
        ("sleeptalk", "onHit") => sleeptalk_hit,
        ("roleplay", "onTryHit") => roleplay_try_hit,
        ("roleplay", "onHit") => roleplay_hit,
        ("stoneaxe", "onAfterSubDamage") => stoneaxe_after_sub,
        ("ceaselessedge", "onAfterSubDamage") => ceaselessedge_after_sub,
        ("rapidspin", "onAfterSubDamage") | ("mortalspin", "onAfterSubDamage") => spin_after_sub,
        ("icespinner", "onAfterSubDamage") => icespinner_after_sub,
        ("shellsidearm", "onAfterSubDamage") | ("shellsidearm", "onHit") => noop_move,
        ("shedtail", "self.onHit") => shedtail_self_hit,

        ("thunder", "onModifyMove") => hurricane_modify,
        ("storedpower", "basePowerCallback") => powertrip_bp,
        ("infernalparade", "basePowerCallback") => hex_bp,
        ("shedtail", "onTryHit") => shedtail_try_hit,
        ("shedtail", "onHit") => shedtail_hit,
        ("endeavor", "damageCallback") => endeavor_damage,
        ("endeavor", "onTryImmunity") => endeavor_immunity,
        ("icespinner", "onAfterHit") => icespinner_after_hit,
        ("ragingbull", "onTryHit") => ragingbull_try_hit,
        ("ragingbull", "onModifyType") => ragingbull_type,
        ("rest", "onTry") => rest_try,
        ("rest", "onHit") => rest_hit,
        ("shellsidearm", "onModifyMove") => shellsidearm_modify,
        ("burnup", "onTryMove") => burnup_try_move,
        ("burnup", "self.onHit") => burnup_self_hit,
        ("facade", "onBasePower") => facade_bp,
        ("ficklebeam", "onBasePower") => ficklebeam_bp,
        ("aurawheel", "onModifyType") => aurawheel_type,
        ("clearsmog", "onHit") => clearsmog_hit,
        ("growth", "onModifyMove") => growth_modify,
        ("mistyexplosion", "onBasePower") => mistyexplosion_bp,
        ("hardpress", "basePowerCallback") => hardpress_bp,
        ("speedswap", "onHit") => speedswap_hit,
        ("simplebeam", "onTryHit") => simplebeam_try_hit,
        ("simplebeam", "onHit") => simplebeam_hit,

        ("lastresort", "onTry") => lastresort_try,
        ("leechseed", "onTryImmunity") => leechseed_immunity,
        ("moonlight", "onHit") | ("morningsun", "onHit") | ("synthesis", "onHit") => weatherheal_hit,
        ("painsplit", "onHit") => painsplit_hit,
        ("pollenpuff", "onTryHit") => pollenpuff_try_hit,
        ("pollenpuff", "onTryMove") => pollenpuff_try_move,
        ("pollenpuff", "onHit") => pollenpuff_hit,
        ("stoneaxe", "onAfterHit") => stoneaxe_after_hit,
        ("ceaselessedge", "onAfterHit") => ceaselessedge_after_hit,
        ("rapidspin", "onAfterHit") | ("mortalspin", "onAfterHit") => spin_after_hit,
        ("burningjealousy", "secondary.onHit") => burningjealousy_secondary,
        ("destinybond", "onPrepareHit") => destinybond_prepare,
        ("afteryou", "onHit") => afteryou_hit,
        ("upperhand", "onTry") => upperhand_try,
        ("lashout", "onBasePower") => lashout_bp,
        ("strengthsap", "onHit") => strengthsap_hit,

        ("acrobatics", "basePowerCallback") => acrobatics_bp,
        ("acupressure", "onHit") => acupressure_hit,
        ("alluringvoice", "secondary.onHit") => alluringvoice_secondary,
        ("allyswitch", "onPrepareHit") => allyswitch_prepare,
        ("allyswitch", "onHit") => allyswitch_hit,
        ("assurance", "basePowerCallback") => assurance_bp,
        ("payback", "basePowerCallback") => payback_bp,
        ("pluck", "onHit") => pluck_hit,
        ("reversal", "basePowerCallback") => reversal_bp,
        ("electroball", "basePowerCallback") => electroball_bp,
        ("metalburst", "damageCallback") => comeuppance_damage,
        ("metalburst", "onTry") => comeuppance_try,
        ("metalburst", "onModifyTarget") => comeuppance_modify_target,
        ("swallow", "onTry") => swallow_try,
        ("swallow", "onHit") => swallow_hit,
        ("dive", "onTryMove") => dive_try_move,
        ("dig", "onTryMove") => charge_try_move,
        ("fly", "onTryMove") => charge_try_move,
        ("beakblast", "priorityChargeCallback") => beakblast_charge,
        ("beakblast", "onAfterMove") => beakblast_after_move,
        ("defog", "onHit") => defog_hit,
        ("topsyturvy", "onHit") => topsyturvy_hit,
        ("copycat", "onHit") => copycat_hit,
        ("comeuppance", "damageCallback") => comeuppance_damage,
        ("comeuppance", "onTry") => comeuppance_try,
        ("comeuppance", "onModifyTarget") => comeuppance_modify_target,
        ("magnetrise", "onTry") => magnetrise_try,
        ("electrify", "onTryHit") => electrify_try_hit,
        ("corrosivegas", "onHit") => corrosivegas_hit,
        ("octolock", "onTryImmunity") => octolock_try_immunity,
        ("uproar", "onTryHit") => uproar_try_hit,
        ("watershuriken", "basePowerCallback") => watershuriken_bp,
        ("gyroball", "basePowerCallback") => gyroball_bp,
        ("flyingpress", "onEffectiveness") => flyingpress_effectiveness,
        ("venoshock", "onBasePower") => venoshock_bp,
        ("fellstinger", "onAfterMoveSecondarySelf") => fellstinger_after,
        ("sparklingaria", "onAfterMove") => sparklingaria_after_move,
        ("futuresight", "onTry") => futuresight_try,
        ("shellsidearm", "onPrepareHit") => shellsidearm_prepare,
        ("focuspunch", "priorityChargeCallback") => focuspunch_charge,
        ("focuspunch", "beforeMoveCallback") => focuspunch_before_move,
        ("mirrorcoat", "beforeTurnCallback") => mirrorcoat_before_turn,
        ("mirrorcoat", "damageCallback") => mirrorcoat_damage,
        ("mirrorcoat", "onTry") => mirrorcoat_try,
        ("counter", "beforeTurnCallback") => counter_before_turn,
        ("counter", "damageCallback") => counter_damage,
        ("counter", "onTry") => counter_try,
        ("avalanche", "basePowerCallback") => avalanche_bp,
        ("gravapple", "onBasePower") => gravapple_bp,
        ("aurawheel", "onTry") => aurawheel_try,
        ("chillyreception", "priorityChargeCallback") => chillyreception_charge,
        ("auroraveil", "onTry") => auroraveil_try,
        ("protect", "onPrepareHit") => stall_prepare,
        ("protect", "onHit") => stall_hit,
        ("detect", "onPrepareHit") => stall_prepare,
        ("detect", "onHit") => stall_hit,
        ("banefulbunker", "onPrepareHit") => stall_prepare,
        ("banefulbunker", "onHit") => stall_hit,
        ("spikyshield", "onPrepareHit") => stall_prepare,
        ("spikyshield", "onHit") => stall_hit,
        ("kingsshield", "onPrepareHit") => stall_prepare,
        ("kingsshield", "onHit") => stall_hit,
        ("endure", "onPrepareHit") => stall_prepare,
        ("endure", "onHit") => stall_hit,
        ("barbbarrage", "onBasePower") => barbbarrage_bp,
        ("batonpass", "onHit") => batonpass_hit,
        ("batonpass", "self.onHit") => batonpass_self,
        ("beatup", "basePowerCallback") => beatup_bp,
        ("beatup", "onModifyMove") => beatup_modify,
        ("bellydrum", "onHit") => bellydrum_hit,
        ("blizzard", "onModifyMove") => blizzard_modify,
        ("brickbreak", "onTryHit") => screens_break,
        ("psychicfangs", "onTryHit") => screens_break,
        ("bugbite", "onHit") => bugbite_hit,
        ("clangoroussoul", "onTry") => clangoroussoul_try,
        ("clangoroussoul", "onTryHit") => clangoroussoul_try_hit,
        ("clangoroussoul", "onHit") => clangoroussoul_hit,
        ("direclaw", "secondary.onHit") => direclaw_secondary,
        ("triattack", "secondary.onHit") => triattack_secondary,
        ("disable", "onTryHit") => disable_try_hit,
        ("doubleshock", "onTryMove") => doubleshock_try_move,
        ("doubleshock", "self.onHit") => doubleshock_self,
        ("electroshot", "onTryMove") => electroshot_try_move,
        ("meteorbeam", "onTryMove") => electroshot_try_move,
        ("phantomforce", "onTryMove") => charge_try_move,
        ("solarbeam", "onTryMove") => solarbeam_try_move,
        ("solarbeam", "onBasePower") => solarbeam_bp,
        ("solarblade", "onTryMove") => solarbeam_try_move,
        ("solarblade", "onBasePower") => solarbeam_bp,
        ("entrainment", "onTryHit") => entrainment_try_hit,
        ("entrainment", "onHit") => entrainment_hit,
        ("eruption", "basePowerCallback") => hp_scaled_bp,
        ("waterspout", "basePowerCallback") => hp_scaled_bp,
        ("expandingforce", "onBasePower") => expandingforce_bp,
        ("expandingforce", "onModifyMove") => expandingforce_modify,
        ("fakeout", "onTry") => first_turn_try,
        ("fakeout", "onDisableMove") => first_turn_disable,
        ("firstimpression", "onTry") => first_turn_try,
        ("firstimpression", "onDisableMove") => first_turn_disable,
        ("finalgambit", "damageCallback") => finalgambit_damage,
        ("followme", "onTry") => doubles_only_try,
        ("ragepowder", "onTry") => doubles_only_try,
        ("freezedry", "onEffectiveness") => freezedry_effectiveness,
        ("grassknot", "basePowerCallback") => weight_bp,
        ("grassknot", "onTryHit") => grassknot_try_hit,
        ("lowkick", "basePowerCallback") => weight_bp,
        ("lowkick", "onTryHit") => heavy_try_hit,
        ("heavyslam", "basePowerCallback") => heavyslam_bp,
        ("heavyslam", "onTryHit") => heavy_try_hit,
        ("heatcrash", "basePowerCallback") => heavyslam_bp,
        ("heatcrash", "onTryHit") => heavy_try_hit,
        ("grassyglide", "onModifyPriority") => grassyglide_priority,
        ("haze", "onHitField") => haze_hit_field,
        ("healingwish", "onTryHit") => healingwish_try_hit,
        ("healpulse", "onHit") => healpulse_hit,
        ("helpinghand", "onTryHit") => helpinghand_try_hit,
        ("hex", "basePowerCallback") => hex_bp,
        ("highjumpkick", "onMoveFail") => highjumpkick_fail,
        ("hurricane", "onModifyMove") => hurricane_modify,
        ("knockoff", "onBasePower") => knockoff_bp,
        ("knockoff", "onAfterHit") => knockoff_after_hit,
        ("lastrespects", "basePowerCallback") => lastrespects_bp,
        ("magicpowder", "onHit") => magicpowder_hit,
        ("soak", "onHit") => soak_hit,
        ("trickortreat", "onHit") => trickortreat_hit,
        ("forestscurse", "onHit") => forestscurse_hit,
        ("partingshot", "onHit") => partingshot_hit,
        ("perishsong", "onHitField") => perishsong_hit_field,
        ("poltergeist", "onTry") => poltergeist_try,
        ("poltergeist", "onTryHit") => poltergeist_try_hit,
        ("powertrip", "basePowerCallback") => powertrip_bp,
        ("psychup", "onHit") => psychup_hit,
        ("quash", "onHit") => quash_hit,
        ("quickguard", "onTry") => willact_try,
        ("quickguard", "onHitSide") => side_stall_hit,
        ("wideguard", "onTry") => willact_try,
        ("wideguard", "onHitSide") => side_stall_hit,
        ("ragefist", "basePowerCallback") => ragefist_bp,
        ("revivalblessing", "onTryHit") => revivalblessing_try_hit,
        ("risingvoltage", "basePowerCallback") => risingvoltage_bp,
        ("round", "basePowerCallback") => round_bp,
        ("round", "onTry") => round_try,
        ("skillswap", "onHit") => skillswap_hit,
        ("steelbeam", "onMoveFail") => steelbeam_fail,
        ("steelroller", "onTry") => steelroller_try,
        ("steelroller", "onHit") => steelroller_hit,
        ("steelroller", "onAfterSubDamage") => steelroller_hit,
        ("stockpile", "onTry") => stockpile_try,
        ("stompingtantrum", "basePowerCallback") => tantrum_bp,
        ("temperflare", "basePowerCallback") => tantrum_bp,
        ("substitute", "onTryHit") => substitute_try_hit,
        ("substitute", "onHit") => substitute_hit,
        ("suckerpunch", "onTry") => suckerpunch_try,
        ("thunderclap", "onTry") => suckerpunch_try,
        ("superfang", "damageCallback") => superfang_damage,
        ("ruination", "damageCallback") => superfang_damage,
        ("terrainpulse", "onModifyType") => terrainpulse_type,
        ("terrainpulse", "onModifyMove") => terrainpulse_modify,
        ("throatchop", "secondary.onHit") => throatchop_secondary,
        ("spiritshackle", "secondary.onHit") => spiritshackle_secondary,
        ("trick", "onTryImmunity") => trick_immunity,
        ("trick", "onHit") => trick_hit,
        ("switcheroo", "onTryImmunity") => trick_immunity,
        ("switcheroo", "onHit") => trick_hit,
        ("tripleaxel", "basePowerCallback") => tripleaxel_bp,
        ("weatherball", "onModifyType") => weatherball_type,
        ("weatherball", "onModifyMove") => weatherball_modify,
        ("yawn", "onTryHit") => yawn_try_hit,
        ("struggle", "onModifyMove") => struggle_modify_move,
        _ => return None,
    };
    Some(f)
}

/// the move whose handler is running (this.effect), else the move argument
fn this_move(a: &HArgs) -> u16 {
    if a.this_ref.is_move() { a.this_ref.mv } else { a.effect.mv }
}
fn base_bp(b: &Battle, a: &HArgs) -> f64 {
    b.mv(this_move(a)).base_power
}

// ------------------------------------------------------------------
fn acrobatics_bp(b: &mut Battle, a: &HArgs) -> Val {
    let user = tgt(a);
    let bp = base_bp(b, a);
    if b.p(user).item == b.d.c.empty {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn acupressure_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let stats: Vec<u8> = (0..7u8).filter(|&k| b.p(t).boosts[k as usize] < 6).collect();
    if stats.is_empty() {
        return Val::False;
    }
    let s = stats[b.prng.sample_index(stats.len())];
    let bo = Battle::boosts1(s, 2);
    b.boost(bo, None, None, EffRef::NONE, false, false);
    Val::Undef
}
fn alluringvoice_secondary(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).stats_raised_this_turn {
        let c = b.d.c.confusion;
        b.add_volatile(t, c, src(a), a.effect, None);
    }
    Val::Undef
}
fn allyswitch_prepare(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let c = b.cond("allyswitch");
    b.add_volatile(m, c, None, EffRef::NONE, None)
}
fn allyswitch_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let new_pos = if b.p(m).position == 0 { 1 } else { 0 };
    let other = b.active_at(m.s as usize, new_pos);
    let ok = other.map(|o| !b.p(o).fainted).unwrap_or(false);
    if !ok {
        add!(b, "-fail", m, "move: Ally Switch");
        b.attr_last_move(&["[still]"]);
        return Val::Empty;
    }
    b.swap_position(m, new_pos as u8, "[from] move: Ally Switch");
    Val::Undef
}
fn assurance_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Num(base_bp(b, a)) };
    let bp = base_bp(b, a);
    if b.p(t).hurt_this_turn.map(|h| h != 0).unwrap_or(false) {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn auroraveil_try(b: &mut Battle, _: &HArgs) -> Val {
    Val::from_bool(b.is_weather(&["hail", "snowscape"]))
}
fn stall_prepare(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.will_act().is_none() {
        return Val::False;
    }
    b.run_event(Ev::StallMove, Arg::Mon(m), None, EffRef::NONE, Val::Undef)
}
fn stall_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let st = b.d.c.stall;
    b.add_volatile(m, st, None, EffRef::NONE, None);
    Val::Undef
}
fn barbbarrage_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let s = b.p(t).status;
    if s == b.d.c.psn || s == b.d.c.tox {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn batonpass_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.can_switch(t.s as usize) == 0 || b.has_vol(t, "commanded") {
        b.attr_last_move(&["[still]"]);
        add!(b, "-fail", t);
        return Val::Empty;
    }
    Val::Undef
}
fn batonpass_self(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).skip_before_switch_out = true;
    Val::Undef
}
fn beatup_modify(b: &mut Battle, a: &HArgs) -> Val {
    let user = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let s = user.s as usize;
    let allies: Vec<Mon> = b.sides[s]
        .pokemon
        .iter()
        .map(|&i| Mon { s: user.s, i })
        .filter(|&x| x == user || (!b.p(x).fainted && b.p(x).status == b.d.c.empty))
        .collect();
    let n = allies.len() as u8;
    b.mvm(mv).allies = allies;
    b.mvm(mv).multihit = Some((n, n));
    Val::Undef
}
fn beatup_bp(b: &mut Battle, a: &HArgs) -> Val {
    let mv = this_move(a);
    if b.mv(mv).allies.is_empty() {
        return Val::Num(5.0);
    }
    let ally = b.mvm(mv).allies.remove(0);
    let sp = b.d.species_id(&b.p(ally).set.species).unwrap_or(0);
    let atk = b.d.species[sp].base[1];
    Val::Num(5.0 + (atk as f64 / 10.0).floor())
}
fn bellydrum_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let p = b.p(t);
    if p.hp as f64 <= p.maxhp as f64 / 2.0 || p.boosts[0] >= 6 || p.maxhp == 1 {
        return Val::False;
    }
    let d = b.p(t).maxhp as f64 / 2.0;
    b.direct_damage(d, None, None, EffRef::NONE);
    let bo = Battle::boosts1(B_ATK, 12);
    b.boost(bo, Some(t), None, EffRef::NONE, false, false);
    Val::Undef
}
fn blizzard_modify(b: &mut Battle, a: &HArgs) -> Val {
    if b.is_weather(&["hail", "snowscape"]) {
        b.mvm(this_move(a)).accuracy = None;
    }
    Val::Undef
}
fn screens_break(b: &mut Battle, a: &HArgs) -> Val {
    // onTryHit(pokemon): the target
    let t = tgt(a);
    for id in ["reflect", "lightscreen", "auroraveil"] {
        if let Some(c) = b.d.cond(id) {
            b.remove_side_condition(t.s, c);
        }
    }
    Val::Undef
}
fn bugbite_hit(b: &mut Battle, a: &HArgs) -> Val {
    steal_eat(b, a, "Bug Bite")
}
fn pluck_hit(b: &mut Battle, a: &HArgs) -> Val {
    steal_eat(b, a, "Pluck")
}
fn steal_eat(b: &mut Battle, a: &HArgs, move_name: &str) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let item = b.p(t).item;
    let is_berry = b.eff(item).item.as_ref().map(|i| i.is_berry).unwrap_or(false);
    if b.p(s).hp > 0 && is_berry && b.take_item(t, Some(s)).is_some() {
        let n = b.eff(item).name.clone();
        let ss = b.mon_str(s);
        add!(b, "-enditem", t, n, "[from] stealeat", format!("[move] {move_name}"), format!("[of] {ss}"));
        let r = b.single_event(Ev::Eat, EffRef::eff(item), StateLoc::Item(t), Arg::Mon(s), Arg::Mon(s), a.effect, Val::Undef);
        if r.truthy() {
            b.run_event(Ev::EatItem, Arg::Mon(s), Some(s), a.effect, Val::Eff(item));
        }
        if b.eff(item).has(b.d.cbs(Ev::Eat)[P_ON]) {
            b.pm(s).ate_berry = true;
        }
    }
    Val::Undef
}
fn clangoroussoul_try(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    let p = b.p(s);
    if p.hp as f64 <= p.maxhp as f64 * 33.0 / 100.0 || p.maxhp == 1 {
        return Val::False;
    }
    Val::Undef
}
fn clangoroussoul_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let mv = this_move(a);
    let Some(bo) = b.mv(mv).boosts else { return Val::Undef };
    if !b.boost(bo, None, None, EffRef::NONE, false, false).truthy() {
        return Val::Null;
    }
    b.mvm(mv).boosts = None;
    Val::Undef
}
fn clangoroussoul_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let d = b.p(m).maxhp as f64 * 33.0 / 100.0;
    b.direct_damage(d, None, None, EffRef::NONE);
    Val::Undef
}
fn random_status_secondary(b: &mut Battle, a: &HArgs, opts: [&str; 3]) -> Val {
    let t = tgt(a);
    let i = b.prng.sample_index(3);
    let st = b.cond(opts[i]);
    b.try_set_status(t, st, src(a), EffRef::NONE);
    Val::Undef
}
fn direclaw_secondary(b: &mut Battle, a: &HArgs) -> Val {
    random_status_secondary(b, a, ["psn", "par", "slp"])
}
fn triattack_secondary(b: &mut Battle, a: &HArgs) -> Val {
    random_status_secondary(b, a, ["brn", "par", "frz"])
}
fn disable_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    match b.p(t).last_move {
        None => Val::False,
        Some(lm) if b.d.moves[lm as usize].id == "struggle" => Val::False,
        _ => Val::Undef,
    }
}
fn doubleshock_try_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_type(m, "Electric") {
        return Val::Undef;
    }
    add!(b, "-fail", m, "move: Double Shock");
    b.attr_last_move(&["[still]"]);
    Val::Null
}
fn doubleshock_self(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let elec = b.d.type_id("Electric");
    let types: Vec<TypeId> = b.get_types(m, true).into_iter().map(|t| if t == elec { NO_TYPE } else { t }).collect();
    b.set_type(m, types, false);
    let shown: Vec<String> = b.get_types(m, false).iter().map(|&t| b.d.type_name(t).to_string()).collect();
    add!(b, "-start", m, "typechange", shown.join("/"), "[from] move: Double Shock");
    Val::Undef
}
/// shared by the two-turn moves: returns Some(()) when the charge turn was set up
fn charge_turn(b: &mut Battle, a: &HArgs, skip_in_weather: Option<&[&str]>, boost_spa: bool, message: bool) -> Val {
    let attacker = tgt(a);
    let defender = src(a);
    let mv = this_move(a);
    let id = b.move_def(mv).id.clone();
    if b.remove_volatile_id(attacker, &id) {
        return Val::Undef;
    }
    let n = b.move_def(mv).name.clone();
    add!(b, "-prepare", attacker, n.clone());
    if boost_spa {
        let bo = Battle::boosts1(B_SPA, 1);
        b.boost(bo, Some(attacker), Some(attacker), a.effect, false, false);
    }
    if let Some(ws) = skip_in_weather {
        let w = b.effective_weather(attacker, EffRef::NONE, message);
        if ws.contains(&w) {
            b.attr_last_move(&["[still]"]);
            if b.log.is_some() {
                let ms = b.mon_str(attacker);
                let ds = defender.map(|d| b.mon_str(d)).unwrap_or_else(|| "null".into());
                b.add_move(vec!["-anim".into(), ms, n, ds]);
            }
            return Val::Undef;
        }
    }
    if !b.run_event(Ev::ChargeMove, Arg::Mon(attacker), defender, a.effect, Val::Undef).truthy() {
        return Val::Undef;
    }
    let c = b.d.c.twoturnmove;
    b.add_volatile(attacker, c, defender, EffRef::NONE, None);
    Val::Null
}
fn electroshot_try_move(b: &mut Battle, a: &HArgs) -> Val {
    let id = b.move_def(this_move(a)).id.clone();
    if id == "meteorbeam" {
        return charge_turn(b, a, None, true, false);
    }
    charge_turn(b, a, Some(&["raindance", "primordialsea"]), true, false)
}
fn charge_try_move(b: &mut Battle, a: &HArgs) -> Val {
    charge_turn(b, a, None, false, false)
}
fn solarbeam_try_move(b: &mut Battle, a: &HArgs) -> Val {
    charge_turn(b, a, Some(&["sunnyday", "desolateland"]), false, true)
}
fn solarbeam_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let w = b.effective_weather(m, EffRef::NONE, false);
    if matches!(w, "raindance" | "primordialsea" | "sandstorm" | "hail" | "snowscape") {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn entrainment_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if t == s || b.has_vol(t, "dynamax") {
        return Val::False;
    }
    let (ta, sa) = (b.p(t).ability, b.p(s).ability);
    let cant = b.eff(ta).ability.as_ref().map(|x| x.cantsuppress).unwrap_or(false);
    let noentrain = b.eff(sa).ability.as_ref().map(|x| x.failskillswap).unwrap_or(false) && b.eff(sa).id != "trace";
    let _ = noentrain;
    if ta == sa || cant || b.eff(ta).id == "truant" {
        return Val::False;
    }
    Val::Undef
}
fn entrainment_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let sa = b.p(s).ability;
    let r = b.set_ability(t, sa, Some(s), EffRef::NONE, false, false);
    if !r.truthy() {
        return r;
    }
    Val::Undef
}
fn hp_scaled_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    Val::Num(base_bp(b, a) * b.p(m).hp as f64 / b.p(m).maxhp as f64)
}
fn expandingforce_bp(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.is_terrain("psychicterrain", None) && b.is_grounded(s, false) == Some(true) {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn expandingforce_modify(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.is_terrain("psychicterrain", None) && b.is_grounded(s, false) == Some(true) {
        b.mvm(this_move(a)).target = MoveTarget::AllAdjacentFoes;
    }
    Val::Undef
}
fn first_turn_try(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.p(s).active_move_actions > 1 {
        let n = b.move_def(this_move(a)).name.clone();
        b.hint(&format!("{n} only works on your first turn out."), false);
        return Val::False;
    }
    Val::Undef
}
fn first_turn_disable(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).active_move_actions != 0 {
        let id = b.mv(this_move(a)).def;
        b.disable_move(m, id, false, None);
    }
    Val::Undef
}
fn finalgambit_damage(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let d = b.p(m).hp;
    b.faint(m, None, EffRef::NONE);
    Val::Num(d as f64)
}
fn doubles_only_try(_: &mut Battle, _: &HArgs) -> Val {
    Val::True
}
fn freezedry_effectiveness(b: &mut Battle, a: &HArgs) -> Val {
    if a.source == Arg::Type(b.d.type_id("Water")) {
        return Val::Num(1.0);
    }
    Val::Undef
}
fn weight_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Num(20.0) };
    let w = b.get_weight(t);
    let bp = if w >= 2000.0 {
        120.0
    } else if w >= 1000.0 {
        100.0
    } else if w >= 500.0 {
        80.0
    } else if w >= 250.0 {
        60.0
    } else if w >= 100.0 {
        40.0
    } else {
        20.0
    };
    Val::Num(bp)
}
fn heavyslam_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(t) = src(a) else { return Val::Num(40.0) };
    let tw = b.get_weight(t);
    let pw = b.get_weight(m);
    let bp = if pw >= tw * 5.0 {
        120.0
    } else if pw >= tw * 4.0 {
        100.0
    } else if pw >= tw * 3.0 {
        80.0
    } else if pw >= tw * 2.0 {
        60.0
    } else {
        40.0
    };
    Val::Num(bp)
}
fn grassknot_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "dynamax") {
        if let Some(s) = src(a) {
            add!(b, "-fail", s, "move: Grass Knot", "[from] Dynamax");
        }
        b.attr_last_move(&["[still]"]);
        return Val::Null;
    }
    Val::Undef
}
fn heavy_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "dynamax") {
        if let Some(s) = src(a) {
            add!(b, "-fail", s, "Dynamax");
        }
        b.attr_last_move(&["[still]"]);
        return Val::Null;
    }
    Val::Undef
}
fn grassyglide_priority(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.is_terrain("grassyterrain", None) && b.is_grounded(s, false) == Some(true) {
        return Val::Num(a.relay.num() + 1.0);
    }
    Val::Undef
}
fn haze_hit_field(b: &mut Battle, _: &HArgs) -> Val {
    add!(b, "-clearallboost");
    for m in b.all_active() {
        b.pm(m).boosts = [0; 7];
    }
    Val::Undef
}
fn healingwish_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.can_switch(s.s as usize) == 0 {
        b.attr_last_move(&["[still]"]);
        add!(b, "-fail", s);
        return Val::Empty;
    }
    Val::Undef
}
fn healpulse_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let amount = if b.has_ability(s, "megalauncher") {
        b.modify(b.p(t).base_maxhp as f64, 0.75)
    } else {
        (b.p(t).base_maxhp as f64 * 0.5).ceil()
    };
    let ok = b.heal(amount, None, None, EffRef::NONE).truthy();
    if !ok {
        add!(b, "-fail", t, "heal");
        return Val::Empty;
    }
    Val::True
}
fn helpinghand_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if !b.p(t).newly_switched && b.will_move(t).is_none() {
        return Val::False;
    }
    Val::Undef
}
fn hex_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Num(base_bp(b, a)) };
    let bp = base_bp(b, a);
    if b.p(t).status != b.d.c.empty || b.has_ability(t, "comatose") {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn highjumpkick_fail(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    let d = b.p(s).base_maxhp as f64 / 2.0;
    let c = b.d.cond("highjumpkick").unwrap_or(b.d.c.empty);
    b.damage(d, Some(s), Some(s), EffRef::eff(c), false);
    Val::Undef
}
fn hurricane_modify(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let w = b.effective_weather(t, EffRef::NONE, false);
    match w {
        "raindance" | "primordialsea" => b.mvm(this_move(a)).accuracy = None,
        "sunnyday" | "desolateland" => b.mvm(this_move(a)).accuracy = Some(50.0),
        _ => {}
    }
    Val::Undef
}
fn knockoff_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let item = b.p(t).item;
    let r = b.single_event(Ev::TakeItem, EffRef::eff(item), StateLoc::Item(t), Arg::Mon(t), Arg::Mon(t), a.effect, Val::Eff(item));
    if !r.truthy() {
        return Val::Undef;
    }
    if item != b.d.c.empty {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn knockoff_after_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let s = src(a);
    if let Some(item) = b.take_item(t, None) {
        let n = b.eff(item).name.clone();
        let ss = s.map(|x| b.mon_str(x)).unwrap_or_default();
        add!(b, "-enditem", t, n, "[from] move: Knock Off", format!("[of] {ss}"));
    }
    Val::Undef
}
fn lastrespects_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    Val::Num(50.0 + 50.0 * b.sides[m.s as usize].total_fainted as f64)
}
fn set_single_type(b: &mut Battle, a: &HArgs, ty: &str, fail_msg: bool) -> Val {
    let t = tgt(a);
    let tid = b.d.type_id(ty);
    let types = b.get_types(t, false);
    if (types.len() == 1 && types[0] == tid) || !b.set_type(t, vec![tid], false) {
        if fail_msg {
            add!(b, "-fail", t);
            return Val::Null;
        }
        return Val::False;
    }
    add!(b, "-start", t, "typechange", ty);
    Val::Undef
}
fn add_type_hit(b: &mut Battle, a: &HArgs, ty: &str, from: &str) -> Val {
    // (the Curse glitch of Trick-or-Treat is not modelled)
    let t = tgt(a);
    if b.has_type(t, ty) {
        return Val::False;
    }
    let tid = b.d.type_id(ty);
    b.pm(t).added_type = tid;
    add!(b, "-start", t, "typeadd", ty, from);
    Val::Undef
}
fn trickortreat_hit(b: &mut Battle, a: &HArgs) -> Val {
    add_type_hit(b, a, "Ghost", "[from] move: Trick-or-Treat")
}
fn forestscurse_hit(b: &mut Battle, a: &HArgs) -> Val {
    add_type_hit(b, a, "Grass", "[from] move: Forest's Curse")
}
fn magicpowder_hit(b: &mut Battle, a: &HArgs) -> Val {
    set_single_type(b, a, "Psychic", false)
}
fn soak_hit(b: &mut Battle, a: &HArgs) -> Val {
    set_single_type(b, a, "Water", true)
}
fn partingshot_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let mut bo = SparseBoosts::default();
    bo.set(B_ATK, -1);
    bo.set(B_SPA, -1);
    let ok = b.boost(bo, Some(t), src(a), EffRef::NONE, false, false);
    if !ok.truthy() && !b.has_ability(t, "mirrorarmor") {
        b.mvm(this_move(a)).self_switch = SelfSwitch::No;
    }
    Val::Undef
}
fn perishsong_hit_field(b: &mut Battle, a: &HArgs) -> Val {
    let Some(source) = src(a) else { return Val::Undef };
    let mut result = false;
    let mut message = false;
    for m in b.all_active() {
        if b.run_event(Ev::Invulnerability, Arg::Mon(m), Some(source), a.effect, Val::Undef) == Val::False {
            add!(b, "-miss", source, m);
            result = true;
        } else if b.run_event(Ev::TryHit, Arg::Mon(m), Some(source), a.effect, Val::Undef) == Val::Null {
            result = true;
        } else if !b.has_vol(m, "perishsong") {
            let c = b.cond("perishsong");
            b.add_volatile(m, c, None, EffRef::NONE, None);
            add!(b, "-start", m, "perish3", "[silent]");
            result = true;
            message = true;
        }
    }
    if !result {
        return Val::False;
    }
    if message {
        add!(b, "-fieldactivate", "move: Perish Song");
    }
    Val::Undef
}
fn poltergeist_try(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::False };
    Val::from_bool(b.p(t).item != b.d.c.empty)
}
fn poltergeist_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let n = b.eff(b.p(t).item).name.clone();
    add!(b, "-activate", t, "move: Poltergeist", n);
    Val::Undef
}
fn powertrip_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    Val::Num(base_bp(b, a) + 20.0 * b.positive_boosts(m) as f64)
}
fn psychup_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    b.pm(s).boosts = b.p(t).boosts;
    let vols = ["dragoncheer", "focusenergy", "gmaxchistrike", "laserfocus"];
    for v in vols {
        b.remove_volatile_id(s, v);
    }
    for v in vols {
        if b.has_vol(t, v) {
            let c = b.cond(v);
            b.add_volatile(s, c, None, EffRef::NONE, None);
            if v == "dragoncheer" {
                let hd = b.volatile(t, c).map(|x| x.get(K::HasDragonType)).unwrap_or(Val::Undef);
                if let Some(x) = b.volatile_mut(s, c) {
                    x.set(K::HasDragonType, hd);
                }
            }
        }
    }
    add!(b, "-copyboost", s, t, "[from] move: Psych Up");
    Val::Undef
}
fn quash_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(qi) = b.will_move(t) else { return Val::False };
    b.queue[qi].order = 201.0;
    add!(b, "-activate", t, "move: Quash");
    Val::Undef
}
fn willact_try(b: &mut Battle, _: &HArgs) -> Val {
    Val::from_bool(b.will_act().is_some())
}
fn side_stall_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    let st = b.d.c.stall;
    b.add_volatile(s, st, None, EffRef::NONE, None);
    Val::Undef
}
fn ragefist_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    Val::Num((50.0 + 50.0 * b.p(m).times_attacked as f64).min(350.0))
}
fn revivalblessing_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    let any = b.sides[s.s as usize].pokemon.iter().any(|&i| b.p(Mon { s: s.s, i }).fainted);
    if !any {
        return Val::False;
    }
    Val::Undef
}
fn risingvoltage_bp(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    let Some(t) = src(a) else { return Val::Num(base_bp(b, a)) };
    let bp = base_bp(b, a);
    if b.is_terrain("electricterrain", None) && b.is_grounded(t, false) == Some(true) {
        if s.s != t.s {
            let n = b.move_def(this_move(a)).name.clone();
            b.hint(&format!("{n}'s BP doubled on grounded target."), false);
        }
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn round_bp(b: &mut Battle, a: &HArgs) -> Val {
    let mv = this_move(a);
    let bp = base_bp(b, a);
    if b.mv(mv).source_effect.map(|e| b.eff(e).id == "round").unwrap_or(false) {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn round_try(b: &mut Battle, a: &HArgs) -> Val {
    let round = b.mv(this_move(a)).eff;
    for qi in 0..b.queue.len() {
        let act = &b.queue[qi];
        if act.pokemon.is_none() || act.mv == NONE16 {
            continue;
        }
        if b.mv(act.mv).eff == round {
            b.prioritize_action(qi, Some(round));
            return Val::Undef;
        }
    }
    Val::Undef
}
fn skillswap_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    b.skill_swap(s, t)
}
fn steelbeam_fail(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    if b.mv(this_move(a)).multihit.is_some() {
        return Val::Undef;
    }
    let d = (b.p(s).maxhp as f64 / 2.0).round();
    let c = b.d.cond("steelbeam").unwrap_or(b.d.c.empty);
    b.damage(d, Some(s), Some(s), EffRef::eff(c), false);
    Val::Undef
}
fn steelroller_try(b: &mut Battle, _: &HArgs) -> Val {
    // !this.field.isTerrain(''): effectiveTerrain() === '' means no (effective) terrain
    let t = b.effective_terrain(None);
    Val::from_bool(!t.is_empty())
}
fn steelroller_hit(b: &mut Battle, _: &HArgs) -> Val {
    b.clear_terrain();
    Val::Undef
}
fn stockpile_try(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if let Some(c) = b.d.cond("stockpile") {
        if b.volatile(s, c).map(|v| v.num(K::Layers) >= 3.0).unwrap_or(false) {
            return Val::False;
        }
    }
    Val::Undef
}
fn tantrum_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let bp = base_bp(b, a);
    if b.p(m).move_last_turn_result == Val::False {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn substitute_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.has_volatile(s, b.d.c.substitute) {
        add!(b, "-fail", s, "move: Substitute");
        return Val::Empty;
    }
    let p = b.p(s);
    if p.hp as f64 <= p.maxhp as f64 / 4.0 || p.maxhp == 1 {
        add!(b, "-fail", s, "move: Substitute", "[weak]");
        return Val::Empty;
    }
    Val::Undef
}
fn substitute_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = b.p(t).maxhp as f64 / 4.0;
    b.direct_damage(d, None, None, EffRef::NONE);
    Val::Undef
}
fn suckerpunch_try(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::False };
    let Some(qi) = b.will_move(t) else { return Val::False };
    let act_mv = b.queue[qi].mv;
    if act_mv == NONE16 {
        return Val::False;
    }
    let m = b.mv(act_mv);
    if (m.category == Category::Status && b.move_def(act_mv).id != "mefirst") || b.has_volatile(t, b.d.c.mustrecharge) {
        return Val::False;
    }
    Val::Undef
}
fn superfang_damage(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    Val::Num(clamp_int(b.p(t).hp as f64 / 2.0, Some(1.0), None))
}
fn terrainpulse_type(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_grounded(m, false) != Some(true) {
        return Val::Undef;
    }
    let t = match b.eff(b.field.terrain).id.as_str() {
        "electricterrain" => "Electric",
        "grassyterrain" => "Grass",
        "mistyterrain" => "Fairy",
        "psychicterrain" => "Psychic",
        _ => return Val::Undef,
    };
    let tid = b.d.type_id(t);
    b.mvm(this_move(a)).type_ = tid;
    Val::Undef
}
fn terrainpulse_modify(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.field.terrain != b.d.c.empty && b.is_grounded(m, false) == Some(true) {
        b.mvm(this_move(a)).base_power *= 2.0;
    }
    Val::Undef
}
fn throatchop_secondary(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let c = b.cond("throatchop");
    b.add_volatile(t, c, None, EffRef::NONE, None);
    Val::Undef
}
fn spiritshackle_secondary(b: &mut Battle, a: &HArgs) -> Val {
    // onHit(target, source, move): if (source.isActive) target.addVolatile('trapped', source, move, 'trapper')
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if b.p(s).is_active {
        let (c, l) = (b.cond("trapped"), b.cond("trapper"));
        b.add_volatile(t, c, Some(s), EffRef::NONE, Some(l));
    }
    Val::Undef
}
fn trick_immunity(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    Val::from_bool(!b.has_ability(t, "stickyhold"))
}
fn trick_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let empty = b.d.c.empty;
    let yours = b.take_item(t, Some(s));
    let mine = b.take_item(s, None);
    let your_failed = yours.is_none() && b.p(t).item != empty;
    let my_failed = mine.is_none() && b.p(s).item != empty;
    if your_failed || my_failed || (yours.is_none() && mine.is_none()) {
        if let Some(y) = yours {
            b.pm(t).item = y;
        }
        if let Some(m) = mine {
            b.pm(s).item = m;
        }
        return Val::False;
    }
    let ok1 = mine.map(|m| b.single_event(Ev::TakeItem, EffRef::eff(m), StateLoc::Item(s), Arg::Mon(t), Arg::Mon(s), a.effect, Val::Eff(m)).truthy()).unwrap_or(true);
    let ok2 = yours.map(|y| b.single_event(Ev::TakeItem, EffRef::eff(y), StateLoc::Item(t), Arg::Mon(s), Arg::Mon(t), a.effect, Val::Eff(y)).truthy()).unwrap_or(true);
    if !ok1 || !ok2 {
        if let Some(y) = yours {
            b.pm(t).item = y;
        }
        if let Some(m) = mine {
            b.pm(s).item = m;
        }
        return Val::False;
    }
    let ts = b.mon_str(t);
    add!(b, "-activate", s, "move: Trick", format!("[of] {ts}"));
    if let Some(m) = mine {
        b.set_item(t, m, None, EffRef::NONE);
        let n = b.eff(m).name.clone();
        add!(b, "-item", t, n, "[from] move: Trick");
    } else if let Some(y) = yours {
        let n = b.eff(y).name.clone();
        add!(b, "-enditem", t, n, "[silent]", "[from] move: Trick");
    }
    if let Some(y) = yours {
        b.set_item(s, y, None, EffRef::NONE);
        let n = b.eff(y).name.clone();
        add!(b, "-item", s, n, "[from] move: Trick");
    } else if let Some(m) = mine {
        let n = b.eff(m).name.clone();
        add!(b, "-enditem", s, n, "[silent]", "[from] move: Trick");
    }
    Val::Undef
}
fn tripleaxel_bp(b: &mut Battle, a: &HArgs) -> Val {
    Val::Num(20.0 * b.mv(this_move(a)).hit as f64)
}
fn weatherball_type(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let t = match b.effective_weather(m, EffRef::NONE, false) {
        "sunnyday" | "desolateland" => "Fire",
        "raindance" | "primordialsea" => "Water",
        "sandstorm" => "Rock",
        "hail" | "snowscape" => "Ice",
        _ => return Val::Undef,
    };
    let tid = b.d.type_id(t);
    b.mvm(this_move(a)).type_ = tid;
    Val::Undef
}
fn weatherball_modify(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if matches!(b.effective_weather(m, EffRef::NONE, false), "sunnyday" | "desolateland" | "raindance" | "primordialsea" | "sandstorm" | "hail" | "snowscape") {
        b.mvm(this_move(a)).base_power *= 2.0;
    }
    Val::Undef
}
fn yawn_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).status != b.d.c.empty || !b.run_status_immunity(t, "slp", false) {
        return Val::False;
    }
    Val::Undef
}
fn struggle_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.mvm(this_move(a)).type_ = NO_TYPE;
    add!(b, "-activate", m, "move: Struggle");
    Val::Undef
}

// ------------------------------------------------------------------ added for the M-A / M-B team pool
fn lastresort_try(b: &mut Battle, a: &HArgs) -> Val {
    // onTry(source): a.target = the user
    let u = tgt(a);
    let slots = &b.p(u).move_slots;
    if slots.len() < 2 {
        return Val::False;
    }
    let lr = b.d.move_id("lastresort").unwrap_or(NONE16);
    let mut has = false;
    for sl in slots.iter() {
        if sl.id == lr {
            has = true;
            continue;
        }
        if !sl.used {
            return Val::False;
        }
    }
    if has { Val::Undef } else { Val::False }
}
fn leechseed_immunity(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_type(t, "Grass") { Val::False } else { Val::True }
}
fn weatherheal_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let factor = match b.effective_weather(m, EffRef::NONE, true) {
        "sunnyday" | "desolateland" => 0.667,
        "raindance" | "primordialsea" | "sandstorm" | "hail" | "snowscape" => 0.25,
        _ => 0.5,
    };
    let amt = b.modify(b.p(m).maxhp as f64, factor);
    let ok = b.heal(amt, None, None, EffRef::NONE).truthy();
    if !ok {
        add!(b, "-fail", m, "heal");
        return Val::Empty;
    }
    Val::True
}
fn painsplit_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    let thp = b.p(t).hp as f64;
    let mut avg = ((thp + b.p(u).hp as f64) / 2.0).floor();
    if avg == 0.0 {
        avg = 1.0;
    }
    let change = thp - avg;
    let th = b.p(t).hp as f64;
    b.sethp(t, th - change);
    add!(b, "-sethp", t, Health(t), "[from] move: Pain Split", "[silent]");
    b.sethp(u, avg);
    add!(b, "-sethp", u, Health(u), "[from] move: Pain Split");
    Val::Undef
}
fn pollenpuff_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if b.is_ally(u, Some(t)) {
        let mv = this_move(a);
        b.mvm(mv).base_power = 0.0;
        b.mvm(mv).infiltrates = true;
    }
    Val::Undef
}
fn pollenpuff_try_move(b: &mut Battle, a: &HArgs) -> Val {
    // onTryMove(source, target): a.target = the user, a.source = the target
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Undef };
    if b.is_ally(u, Some(t)) && b.has_vol(u, "healblock") {
        return Val::False;
    }
    Val::Undef
}
fn pollenpuff_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if b.is_ally(u, Some(t)) {
        let amt = (b.p(t).base_maxhp as f64 * 0.5).floor();
        if !b.heal(amt, None, None, EffRef::NONE).truthy() {
            return Val::Empty;
        }
    }
    Val::Undef
}
fn hazard_after_hit(b: &mut Battle, a: &HArgs, cond: &str) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.mv(this_move(a)).has_sheer_force {
        return Val::Undef;
    }
    let c = b.cond(cond);
    b.add_side_condition(1 - u.s, c, Some(u), EffRef::NONE);
    Val::Undef
}
fn stoneaxe_after_hit(b: &mut Battle, a: &HArgs) -> Val {
    hazard_after_hit(b, a, "stealthrock")
}
fn ceaselessedge_after_hit(b: &mut Battle, a: &HArgs) -> Val {
    hazard_after_hit(b, a, "spikes")
}
fn spin_after_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.mv(this_move(a)).has_sheer_force {
        return Val::Undef;
    }
    b.remove_volatile_id(u, "leechseed");
    for c in ["spikes", "toxicspikes", "stealthrock", "stickyweb", "gmaxsteelsurge"] {
        if let Some(id) = b.d.cond(c) {
            b.remove_side_condition(u.s, id);
        }
    }
    if b.has_vol(u, "partiallytrapped") {
        b.remove_volatile_id(u, "partiallytrapped");
    }
    Val::Undef
}
fn burningjealousy_secondary(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).stats_raised_this_turn {
        let brn = b.d.c.brn;
        b.try_set_status(t, brn, src(a), a.effect);
    }
    Val::Undef
}
fn destinybond_prepare(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.remove_volatile_id(u, "destinybond") { Val::False } else { Val::True }
}
fn afteryou_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(qi) = b.will_move(t) else { return Val::False };
    b.prioritize_action(qi, None);
    add!(b, "-activate", t, "move: After You");
    Val::Undef
}
fn upperhand_try(b: &mut Battle, a: &HArgs) -> Val {
    // onTry(source, target): a.source = the target
    let Some(t) = src(a) else { return Val::False };
    let Some(qi) = b.will_move(t) else { return Val::False };
    let act_mv = b.queue[qi].mv;
    if act_mv == NONE16 {
        return Val::False;
    }
    let m = b.mv(act_mv);
    if m.priority <= 0.1 || m.category == Category::Status {
        return Val::False;
    }
    Val::Undef
}
fn lashout_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.p(u).stats_lowered_this_turn {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn strengthsap_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let u = src(a);
    if b.p(t).boosts[B_ATK as usize] == -6 {
        return Val::False;
    }
    let atk = b.get_stat(t, 1, false, true);
    let bo = Battle::boosts1(B_ATK, -1);
    let ok = b.boost(bo, Some(t), u, EffRef::NONE, false, true).truthy();
    let healed = b.heal(atk, u, Some(t), EffRef::NONE).truthy();
    if healed || ok { Val::True } else { Val::False }
}

// ------------------------------------------------------------------ batch 2 for the M-A / M-B team pool
fn shedtail_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.can_switch(s.s as usize) == 0 || b.has_vol(s, "commanded") {
        add!(b, "-fail", s);
        return Val::Empty;
    }
    if b.has_volatile(s, b.d.c.substitute) {
        add!(b, "-fail", s, "move: Shed Tail");
        return Val::Empty;
    }
    let p = b.p(s);
    if p.hp as f64 <= (p.maxhp as f64 / 2.0).ceil() {
        add!(b, "-fail", s, "move: Shed Tail", "[weak]");
        return Val::Empty;
    }
    Val::Undef
}
fn shedtail_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = (b.p(t).maxhp as f64 / 2.0).ceil();
    b.direct_damage(d, None, None, EffRef::NONE);
    Val::Undef
}
fn endeavor_damage(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Undef };
    Val::Num(b.p(t).hp as f64 - b.p(u).hp as f64)
}
fn endeavor_immunity(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).hp < b.p(t).hp { Val::True } else { Val::False }
}
fn icespinner_after_hit(b: &mut Battle, _: &HArgs) -> Val {
    b.clear_terrain();
    Val::Undef
}
fn ragingbull_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    for c in ["reflect", "lightscreen", "auroraveil"] {
        let id = b.cond(c);
        b.remove_side_condition(t.s, id);
    }
    Val::Undef
}
fn ragingbull_type(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let ty = match b.d.species[b.p(m).species as usize].name.as_str() {
        "Tauros-Paldea-Combat" => "Fighting",
        "Tauros-Paldea-Blaze" => "Fire",
        "Tauros-Paldea-Aqua" => "Water",
        _ => return Val::Undef,
    };
    let tid = b.d.type_id(ty);
    b.mvm(this_move(a)).type_ = tid;
    Val::Undef
}
fn rest_try(b: &mut Battle, a: &HArgs) -> Val {
    let s = tgt(a);
    if b.p(s).status == b.d.c.slp || b.has_ability(s, "comatose") {
        return Val::False;
    }
    if b.p(s).hp == b.p(s).maxhp {
        add!(b, "-fail", s, "heal");
        return Val::Null;
    }
    if b.has_ability(s, "insomnia") {
        let ss = b.mon_str(s);
        add!(b, "-fail", s, "[from] ability: Insomnia", format!("[of] {ss}"));
        return Val::Null;
    }
    if b.has_ability(s, "vitalspirit") {
        let ss = b.mon_str(s);
        add!(b, "-fail", s, "[from] ability: Vital Spirit", format!("[of] {ss}"));
        return Val::Null;
    }
    Val::Undef
}
fn rest_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let slp = b.d.c.slp;
    let r = b.set_status(t, slp, src(a), a.effect, false);
    if !r.truthy() {
        return r;
    }
    b.sset(StateLoc::Status(t), K::Time, Val::Num(3.0));
    b.sset(StateLoc::Status(t), K::StartTime, Val::Num(3.0));
    let h = b.p(t).maxhp as f64;
    b.heal(h, None, None, EffRef::NONE);
    Val::Undef
}
fn shellsidearm_modify(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Undef };
    let atk = b.get_stat(u, 1, false, true);
    let spa = b.get_stat(u, 3, false, true);
    let def = b.get_stat(t, 2, false, true);
    let spd = b.get_stat(t, 4, false, true);
    let base = ((2.0 * b.p(u).level as f64 / 5.0).floor() + 2.0).floor();
    let physical = ((base * 90.0 * atk).floor() / def).floor();
    let physical = (physical / 50.0).floor();
    let special = ((base * 90.0 * spa).floor() / spd).floor();
    let special = (special / 50.0).floor();
    if physical > special || (physical == special && b.random_chance(1, 2)) {
        let mv = this_move(a);
        b.mvm(mv).category = Category::Physical;
        b.mvm(mv).flags |= mflag::CONTACT;
    }
    Val::Undef
}
fn burnup_try_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.has_type(u, "Fire") {
        return Val::Undef;
    }
    add!(b, "-fail", u, "move: Burn Up");
    b.attr_last_move(&["[still]"]);
    Val::Null
}
fn burnup_self_hit(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let fire = b.d.type_id("Fire");
    let none = b.d.type_id("???");
    let types: Vec<TypeId> = b.get_types(u, true).into_iter().map(|t| if t == fire { none } else { t }).collect();
    b.set_type(u, types, false);
    let shown: Vec<String> = b.get_types(u, false).iter().map(|&t| b.d.type_name(t).to_string()).collect();
    add!(b, "-start", u, "typechange", shown.join("/"), "[from] move: Burn Up");
    Val::Undef
}
fn facade_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let st = b.p(u).status;
    if st != b.d.c.empty && st != b.d.c.slp {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn ficklebeam_bp(b: &mut Battle, a: &HArgs) -> Val {
    if b.random_chance(3, 10) {
        let u = tgt(a);
        b.attr_last_move(&["[anim] Fickle Beam All Out"]);
        add!(b, "-activate", u, "move: Fickle Beam");
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn aurawheel_type(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let hangry = b.d.species[b.p(u).species as usize].name == "Morpeko-Hangry";
    let tid = b.d.type_id(if hangry { "Dark" } else { "Electric" });
    b.mvm(this_move(a)).type_ = tid;
    Val::Undef
}
fn clearsmog_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    b.pm(t).boosts = [0; 7];
    add!(b, "-clearboost", t);
    Val::Undef
}
fn growth_modify(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if matches!(b.effective_weather(u, EffRef::NONE, false), "sunnyday" | "desolateland") {
        let mut bo = Battle::boosts1(B_ATK, 2);
        bo.set(B_SPA, 2);
        b.mvm(this_move(a)).boosts = Some(bo);
    }
    Val::Undef
}
fn mistyexplosion_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.is_terrain("mistyterrain", None) && b.is_grounded(u, false) == Some(true) {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn hardpress_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let hp = b.p(t).hp as f64;
    let max = b.p(t).maxhp as f64;
    let x = (hp * 4096.0 / max).floor();
    let bp = (((100.0 * (100.0 * x) + 2048.0 - 1.0) / 4096.0).floor() / 100.0).floor();
    Val::Num(if bp == 0.0 { 1.0 } else { bp })
}
fn speedswap_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    let ts = b.p(t).stored_stats[5];
    b.pm(t).stored_stats[5] = b.p(u).stored_stats[5];
    b.pm(u).stored_stats[5] = ts;
    let tstr = b.mon_str(t);
    add!(b, "-activate", u, "move: Speed Swap", format!("[of] {tstr}"));
    Val::Undef
}
fn simplebeam_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_ability_any(t, &["simple", "truant"]) {
        return Val::False;
    }
    let ab = b.p(t).ability;
    if b.eff(ab).ability.as_ref().map(|x| x.cantsuppress).unwrap_or(false) {
        return Val::False;
    }
    Val::Undef
}
fn simplebeam_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let simple = b.d.ability("simple");
    let r = b.set_ability(t, simple, None, EffRef::NONE, false, false);
    if !r.truthy() {
        return r;
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 4 (rare moves)
fn noop_move(_: &mut Battle, _: &HArgs) -> Val {
    Val::Undef
}
fn last_slot(b: &Battle, m: Mon) -> Option<(u16, usize)> {
    let lm = b.p(m).last_move?;
    let k = b.p(m).move_slots.iter().position(|s| s.id == lm)?;
    Some((lm, k))
}
fn instruct_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(lm) = b.p(t).last_move else { return Val::False };
    if b.has_vol(t, "dynamax") {
        return Val::False;
    }
    let f = b.d.moves[lm as usize].flags;
    let slot_pp = b.p(t).move_slots.iter().find(|s| s.id == lm).map(|s| s.pp);
    if f & (mflag::FAILINSTRUCT | mflag::CHARGE | mflag::RECHARGE) != 0
        || b.has_vol(t, "beakblast") || b.has_vol(t, "focuspunch") || b.has_vol(t, "shelltrap")
        || slot_pp.map(|p| p <= 0).unwrap_or(false)
    {
        return Val::False;
    }
    if let Some(s) = src(a) {
        let ss = b.mon_str(s);
        add!(b, "-singleturn", t, "move: Instruct", format!("[of] {ss}"));
    }
    let mut act = Action::new(ActionKind::Move);
    act.pokemon = Some(t);
    act.move_id = lm;
    act.target_loc = b.p(t).last_move_target_loc.unwrap_or(0);
    let r = b.resolve_action(act, false);
    if let Some(first) = r.into_iter().next() {
        b.queue.push(first);
        let idx = b.queue.len() - 1;
        b.prioritize_action(idx, None);
    }
    Val::Undef
}
fn curse_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let t = src(a);
    let mv = this_move(a);
    if !b.has_type(u, "Ghost") {
        b.mvm(mv).target = MoveTarget::SelfTarget;
    } else if t.is_none() || (t != Some(u) && b.is_ally(u, t)) {
        b.mvm(mv).target = MoveTarget::RandomNormal;
    }
    Val::Undef
}
fn curse_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if b.has_type(u, "Ghost") && b.has_vol(t, "curse") {
        return Val::False;
    }
    Val::Undef
}
fn curse_hit(b: &mut Battle, a: &HArgs) -> Val {
    let mut t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if !b.has_type(u, "Ghost") {
        let mut bo = Battle::boosts1(B_SPE, -1);
        bo.set(B_ATK, 1);
        bo.set(B_DEF, 1);
        return Val::from_bool(b.boost(bo, Some(u), Some(u), EffRef::NONE, false, false).truthy());
    }
    let d = b.p(u).maxhp as f64 / 2.0;
    b.direct_damage(d, Some(u), Some(u), EffRef::NONE);
    if b.is_ally(u, Some(t)) {
        let Some(r) = b.get_random_target(u, this_move(a)) else { return Val::False };
        t = r;
    }
    if let Some(c) = b.d.cond("curse") {
        if let Some(pos) = b.p(t).volatiles.iter().position(|v| v.0 == c) {
            b.pm(t).volatiles.remove(pos);
        }
        b.add_volatile(t, c, Some(u), EffRef::NONE, None);
    }
    Val::Undef
}
fn transform_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    Val::from_bool(b.transform_into(u, t, EffRef::NONE))
}
fn fling_prepare(b: &mut Battle, a: &HArgs) -> Val {
    // onPrepareHit(target, source, move)
    let Some(u) = src(a) else { return Val::False };
    if b.ignoring_item(u) {
        return Val::False;
    }
    let item = b.p(u).item;
    if item == b.d.c.empty {
        return Val::False;
    }
    let info = b.eff(item).item.clone().unwrap_or_default();
    if !info.mega_stone.is_empty() {
        let bs = &b.d.species[b.p(u).base_species as usize];
        if info.mega_stone.iter().any(|(from, to)| *from == bs.base_species || *from == bs.name || *to == bs.name) {
            return Val::False;
        }
    }
    if info.fling_bp == 0 {
        return Val::False;
    }
    let mv = this_move(a);
    b.mvm(mv).base_power = info.fling_bp as f64;
    b.mvm(mv).source_effect = Some(item);
    // items whose fling is a status / volatile become a SECONDARY of the move (Showdown: move.secondaries.push(...)): a
    // random roll is drawn and Shield Dust / Covert Cloak apply; the berries' and herbs' effects stay in onHit
    let (st, vol) = match b.eff(item).id.as_str() {
        "flameorb" => (Some("brn"), None),
        "toxicorb" => (Some("tox"), None),
        "lightball" => (Some("par"), None),
        "poisonbarb" => (Some("psn"), None),
        "kingsrock" | "razorfang" => (None, Some("flinch")),
        _ => (None, None),
    };
    if st.is_some() || vol.is_some() {
        let sec = HitEffectData {
            chance: None,
            boosts: None,
            status: st.map(|x| b.cond(x)),
            volatile_status: vol.map(|x| b.cond(x)),
            side_condition: None,
            self_: None,
            has_on_hit: false,
            dustproof: false,
            kingsrock: false,
        };
        b.mvm(mv).secondaries.get_or_insert_with(Vec::new).push(sec);
    }
    // the item leaves at the next update (the fling volatile's onUpdate, conditions.rs): after the hit, as in Showdown
    let c = b.cond("fling");
    b.add_volatile(u, c, None, EffRef::NONE, None);
    Val::Undef
}
fn fling_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(item) = b.mv(this_move(a)).source_effect else { return Val::Undef };
    let Some(u) = src(a) else { return Val::Undef };
    match b.eff(item).id.as_str() {
        "sitrusberry" => {
            let h = (b.p(t).base_maxhp as f64 / 4.0).floor();
            b.heal(h, Some(t), Some(u), EffRef::NONE);
        }
        _ => {}
    }
    Val::Undef
}
fn eeriespell_secondary(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).hp <= 0 {
        return Val::Undef;
    }
    let Some(lm) = b.p(t).last_move else { return Val::Undef };
    let n = b.deduct_pp(t, lm, Some(3));
    if n > 0 {
        let name = b.d.moves[lm as usize].name.clone();
        add!(b, "-activate", t, "move: Eerie Spell", name, n.to_string());
    }
    Val::Undef
}
fn thief_after_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).item != b.d.c.empty || b.has_vol(u, "gem") {
        return Val::Undef;
    }
    let Some(item) = b.take_item(t, Some(u)) else { return Val::Undef };
    if !b.set_item(u, item, None, EffRef::NONE) {
        b.pm(t).item = item;
        return Val::Undef;
    }
    let n = b.eff(item).name.clone();
    let ts = b.mon_str(t);
    let us = b.mon_str(u);
    add!(b, "-enditem", t, n.clone(), "[silent]", "[from] move: Thief", format!("[of] {us}"));
    add!(b, "-item", u, n, "[from] move: Thief", format!("[of] {ts}"));
    Val::Undef
}
fn bounce_try_move(b: &mut Battle, a: &HArgs) -> Val {
    charge_turn(b, a, None, false, false)
}
fn supercellslam_fail(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    let d = b.p(s).base_maxhp as f64 / 2.0;
    let c = b.d.cond("supercellslam").unwrap_or(b.d.c.empty);
    b.damage(d, Some(s), Some(s), EffRef::eff(c), false);
    Val::Undef
}
fn noretreat_try(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.has_vol(u, "noretreat") {
        return Val::False;
    }
    if b.has_vol(u, "trapped") {
        b.mvm(this_move(a)).volatile_status = None;
    }
    Val::Undef
}
fn sleeptalk_try(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    Val::from_bool(b.p(u).status == b.d.c.slp || b.has_ability(u, "comatose"))
}
fn sleeptalk_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let ids: Vec<u16> = b.p(m).move_slots.iter().map(|s| s.id).filter(|&id| {
        let f = b.d.moves[id as usize].flags;
        f & (mflag::NOSLEEPTALK | mflag::CHARGE) == 0
    }).collect();
    if ids.is_empty() {
        return Val::False;
    }
    let k = b.random(ids.len() as u32) as usize;
    let nm = b.new_active_move(ids[k]);
    b.use_move_random(nm, m, None);
    Val::Undef
}
fn roleplay_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    let (ta, ua) = (b.p(t).ability, b.p(u).ability);
    if ta == ua {
        return Val::False;
    }
    let fail = b.eff(ta).ability.as_ref().map(|x| x.failroleplay).unwrap_or(false);
    let cant = b.eff(ua).ability.as_ref().map(|x| x.cantsuppress).unwrap_or(false);
    if fail || cant {
        return Val::False;
    }
    Val::Undef
}
fn roleplay_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    let ab = b.p(t).ability;
    let r = b.set_ability(u, ab, Some(t), EffRef::NONE, false, false);
    if !r.truthy() {
        return r;
    }
    Val::Undef
}
fn stoneaxe_after_sub(b: &mut Battle, a: &HArgs) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).hp > 0 {
        return hazard_after_hit(b, a, "stealthrock");
    }
    Val::Undef
}
fn ceaselessedge_after_sub(b: &mut Battle, a: &HArgs) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).hp > 0 {
        return hazard_after_hit(b, a, "spikes");
    }
    Val::Undef
}
fn spin_after_sub(b: &mut Battle, a: &HArgs) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).hp > 0 {
        return spin_after_hit(b, a);
    }
    Val::Undef
}
fn icespinner_after_sub(b: &mut Battle, a: &HArgs) -> Val {
    let Some(u) = src(a) else { return Val::Undef };
    if b.p(u).hp > 0 {
        b.clear_terrain();
    }
    Val::Undef
}
fn shedtail_self_hit(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    b.pm(u).skip_before_switch_out = true;
    Val::Undef
}

fn payback_bp(b: &mut Battle, a: &HArgs) -> Val {
    let bp = base_bp(b, a);
    let Some(t) = src(a) else { return Val::Num(bp) };
    if b.p(t).newly_switched || b.will_move(t).is_some() {
        return Val::Num(bp);
    }
    Val::Num(bp * 2.0)
}
fn avalanche_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let bp = base_bp(b, a);
    let Some(t) = src(a) else { return Val::Num(bp) };
    if b.p(u).attacked_by.iter().any(|x| x.source == t && x.damage > 0 && x.this_turn) {
        return Val::Num(bp * 2.0);
    }
    Val::Num(bp)
}
fn gravapple_bp(b: &mut Battle, _: &HArgs) -> Val {
    if b.field.pseudo_weather.iter().any(|x| b.eff(x.0).id == "gravity") {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn aurawheel_try(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if b.d.species[b.p(u).species as usize].base_species == "Morpeko" {
        return Val::Undef;
    }
    b.attr_last_move(&["[still]"]);
    add!(b, "-fail", u, "move: Aura Wheel");
    Val::Null
}
fn chillyreception_charge(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let c = b.cond("chillyreception");
    b.add_volatile(u, c, None, EffRef::NONE, None);
    Val::Undef
}

// Counter / Mirror Coat: a volatile added before the turn records the last opposing physical / special hit's slot and
// 2x its damage (the condition handlers in conditions.rs); the move hits that slot for that damage.
// The slot is stored as side * 8 + position + 1 (Undef = Showdown's null).
fn reflect_before_turn(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let u = tgt(a);
    let c = b.cond(id);
    b.add_volatile(u, c, None, EffRef::NONE, None);
    Val::Undef
}
fn reflect_damage(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let u = tgt(a);
    let c = b.cond(id);
    if !b.p(u).volatiles.iter().any(|x| x.0 == c) {
        return Val::Num(0.0);
    }
    let d = b.sget(StateLoc::Volatile(u, c), K::Hp).num();
    Val::Num(if d > 0.0 { d } else { 1.0 })
}
fn reflect_try(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let u = tgt(a);
    let c = b.cond(id);
    if !b.p(u).volatiles.iter().any(|x| x.0 == c) {
        return Val::False;
    }
    if b.sget(StateLoc::Volatile(u, c), K::TargetSlot) == Val::Undef {
        return Val::False;
    }
    Val::Undef
}
fn mirrorcoat_before_turn(b: &mut Battle, a: &HArgs) -> Val {
    reflect_before_turn(b, a, "mirrorcoat")
}
fn mirrorcoat_damage(b: &mut Battle, a: &HArgs) -> Val {
    reflect_damage(b, a, "mirrorcoat")
}
fn mirrorcoat_try(b: &mut Battle, a: &HArgs) -> Val {
    reflect_try(b, a, "mirrorcoat")
}
fn counter_before_turn(b: &mut Battle, a: &HArgs) -> Val {
    reflect_before_turn(b, a, "counter")
}
fn counter_damage(b: &mut Battle, a: &HArgs) -> Val {
    reflect_damage(b, a, "counter")
}
fn counter_try(b: &mut Battle, a: &HArgs) -> Val {
    reflect_try(b, a, "counter")
}

fn focuspunch_charge(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let c = b.cond("focuspunch");
    b.add_volatile(u, c, None, EffRef::NONE, None);
    Val::Undef
}
fn focuspunch_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let c = b.cond("focuspunch");
    if b.p(u).volatiles.iter().any(|x| x.0 == c) && b.sget(StateLoc::Volatile(u, c), K::X1).truthy() {
        add!(b, "cant", u, "Focus Punch", "Focus Punch");
        return Val::True;
    }
    Val::Undef
}

fn shellsidearm_prepare(b: &mut Battle, a: &HArgs) -> Val {
    // onPrepareHit(target, source, move)
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    if !b.is_ally(u, Some(t)) {
        let cat = if b.mv(this_move(a)).category == Category::Physical { "Physical" } else { "Special" };
        let attr = format!("[anim] Shell Side Arm {cat}");
        b.attr_last_move(&[attr.as_str()]);
    }
    Val::Undef
}

fn futuresight_try(b: &mut Battle, a: &HArgs) -> Val {
    // onTry(source, target)
    if b.mv(this_move(a)).future_hit {
        return Val::Undef;                  // the delayed hit itself (plain move data, no handlers)
    }
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Undef };
    let c = b.cond("futuremove");
    let pos = b.p(t).position;
    if !b.add_slot_condition(t.s, pos, c, Some(u), EffRef::NONE).truthy() {
        return Val::False;
    }
    let def = b.mv(this_move(a)).def;
    b.sset(StateLoc::Slot(t.s, pos, c), K::Move, Val::MoveId(def));
    if let Some(st) = b.state_mut(StateLoc::Slot(t.s, pos, c)) {
        st.source = Some(u);
    }
    add!(b, "-start", u, "move: Future Sight");
    Val::Empty
}

fn watershuriken_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let bp = base_bp(b, a);
    if b.d.species[b.p(u).species as usize].name == "Greninja-Ash" && b.has_ability(u, "battlebond") && !b.p(u).transformed {
        return Val::Num(bp + 5.0);
    }
    Val::Num(bp)
}
fn gyroball_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Num(1.0) };
    let ts = b.get_stat(t, 5, false, false);
    let us = b.get_stat(u, 5, false, false);
    let mut power = (25.0 * ts / us).floor() + 1.0;
    if !power.is_finite() {
        power = 1.0;
    }
    Val::Num(power.min(150.0))
}
fn flyingpress_effectiveness(b: &mut Battle, a: &HArgs) -> Val {
    let Arg::Type(t) = a.source else { return Val::Undef };
    let flying = b.d.type_id("Flying");
    let e = match b.d.taken_type(flying, t) {
        1 => 1.0,
        2 => -1.0,
        _ => 0.0,
    };
    Val::Num(a.relay.num() + e)
}
fn venoshock_bp(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let st = b.p(t).status;
    if st == b.d.c.psn || st == b.d.c.tox {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn fellstinger_after(b: &mut Battle, a: &HArgs) -> Val {
    // onAfterMoveSecondarySelf(pokemon, target, move)
    let u = tgt(a);
    let t = src(a);
    if t.map(|t| b.p(t).fainted || b.p(t).hp == 0).unwrap_or(true) {
        let mut bo = SparseBoosts::default();
        bo.set(B_ATK, 3);
        let mr = EffRef { eff: b.mv(this_move(a)).eff, mv: this_move(a), part: 0 };
        b.boost(bo, Some(u), Some(u), mr, false, false);
    }
    Val::Undef
}
fn sparklingaria_after_move(b: &mut Battle, a: &HArgs) -> Val {
    // onAfterMove(source, target, move)
    let u = tgt(a);
    let mv = this_move(a);
    let sa = b.cond("sparklingaria");
    let hits = b.mv(mv).hit_targets.clone();
    if b.p(u).fainted || hits.is_empty() || b.mv(mv).has_sheer_force {
        for (m, _) in b.get_all_active(false) {
            b.pm(m).volatiles.retain(|x| x.0 != sa);
        }
        return Val::Undef;
    }
    let n = hits.len();
    for m in hits {
        if m != u && b.p(m).is_active && (b.remove_volatile(m, sa) || n > 1) && b.p(m).status == b.d.c.brn {
            b.cure_status(m, false);
        }
    }
    Val::Undef
}

fn octolock_try_immunity(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    Val::from_bool(b.dex_immunity("trapped", t))
}
fn uproar_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    // wakes every sleeping active Pokemon, ours and the foe's, slot by slot
    let t = tgt(a);
    let slp = b.d.c.slp;
    let (side, foe) = (t.s as usize, 1 - t.s as usize);
    let n = b.sides[side].active.len();
    for i in 0..n {
        if let Some(Some(k)) = b.sides[side].active.get(i).copied() {
            let m = Mon { s: side as u8, i: k };
            if b.p(m).status == slp {
                b.cure_status(m, false);
            }
        }
        if let Some(Some(k)) = b.sides[foe].active.get(i).copied() {
            let m = Mon { s: foe as u8, i: k };
            if b.p(m).status == slp {
                b.cure_status(m, false);
            }
        }
    }
    Val::Undef
}

fn copycat_hit(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(last) = b.last_move else { return Val::Undef };
    if b.mv(last).flags & mflag::bit("failcopycat") != 0 || b.mv(last).is_z_or_max_powered {
        return Val::False;
    }
    let def = b.mv(last).def;
    let nm = b.new_active_move(def);
    b.use_move_random(nm, u, None);
    Val::Undef
}
/// pokemon.getLastDamagedBy(true): the last attacker from the other side that dealt numeric damage
fn last_damaged_by(b: &Battle, m: Mon) -> Option<Attacker> {
    b.p(m).attacked_by.iter().rev().find(|x| matches!(x.damage_value, Val::Num(_)) && !b.is_ally(m, Some(x.source))).cloned()
}
fn comeuppance_damage(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    match last_damaged_by(b, u) {
        Some(x) => {
            let d = x.damage as f64 * 1.5;
            Val::Num(if d != 0.0 { d } else { 1.0 })
        }
        None => Val::Num(0.0),
    }
}
fn comeuppance_try(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if !last_damaged_by(b, u).map(|x| x.this_turn).unwrap_or(false) {
        return Val::False;
    }
    Val::Undef
}
fn comeuppance_modify_target(b: &mut Battle, a: &HArgs) -> Val {
    // onModifyTarget(targetRelayVar, source, target, move): the target becomes whoever is in that attacker's slot
    let u = tgt(a);
    if let Some(x) = last_damaged_by(b, u) {
        return match b.active_at(x.slot.0 as usize, x.slot.1 as usize) {
            Some(t) => Val::Mon(t),
            None => Val::Null,
        };
    }
    Val::Undef
}
fn magnetrise_try(b: &mut Battle, a: &HArgs) -> Val {
    // onTry(source, target, move)
    let u = tgt(a);
    let t = src(a).unwrap_or(u);
    if b.has_vol(t, "smackdown") || b.has_vol(t, "ingrain") {
        return Val::False;
    }
    if b.field.pseudo_weather.iter().any(|x| b.eff(x.0).id == "gravity") {
        let n = b.move_def(this_move(a)).name.clone();
        add!(b, "cant", u, "move: Gravity", n);
        return Val::Null;
    }
    Val::Undef
}
fn electrify_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.will_move(t).is_none() && b.p(t).active_turns != 0 {
        return Val::False;
    }
    Val::Undef
}
fn corrosivegas_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    match b.take_item(t, Some(u)) {
        Some(item) => {
            let n = b.eff(item).name.clone();
            let us = b.mon_str(u);
            add!(b, "-enditem", t, n, "[from] move: Corrosive Gas", format!("[of] {us}"));
        }
        None => {
            add!(b, "-fail", t, "move: Corrosive Gas");
        }
    }
    Val::Undef
}

fn beakblast_charge(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let c = b.cond("beakblast");
    b.add_volatile(u, c, None, EffRef::NONE, None);
    Val::Undef
}
fn beakblast_after_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    b.remove_volatile_id(u, "beakblast");
    Val::Undef
}
fn defog_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(u) = src(a) else { return Val::Undef };
    let mv = this_move(a);
    let mut success = false;
    if !b.has_vol(t, "substitute") || b.mv(mv).infiltrates {
        let bo = Battle::boosts1(B_EVA, -1);
        success = b.boost(bo, None, None, EffRef::NONE, false, false).truthy();
    }
    const REMOVE_ALL: [&str; 5] = ["spikes", "toxicspikes", "stealthrock", "stickyweb", "gmaxsteelsurge"];
    const REMOVE_TARGET: [&str; 10] = ["reflect", "lightscreen", "auroraveil", "safeguard", "mist",
                                       "spikes", "toxicspikes", "stealthrock", "stickyweb", "gmaxsteelsurge"];
    let us = b.mon_str(u);
    for id in REMOVE_TARGET {
        let Some(c) = b.d.cond(id) else { continue };
        if b.remove_side_condition(t.s, c) {
            if !REMOVE_ALL.contains(&id) {
                continue;
            }
            let side = super::conditions::side_str(b, t.s);
            let n = b.eff(c).name.clone();
            add!(b, "-sideend", side, n, "[from] move: Defog", format!("[of] {us}"));
            success = true;
        }
    }
    for id in REMOVE_ALL {
        let Some(c) = b.d.cond(id) else { continue };
        if b.remove_side_condition(u.s, c) {
            let side = super::conditions::side_str(b, u.s);
            let n = b.eff(c).name.clone();
            add!(b, "-sideend", side, n, "[from] move: Defog", format!("[of] {us}"));
            success = true;
        }
    }
    b.clear_terrain();
    Val::from_bool(success)
}
fn topsyturvy_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let mut success = false;
    for k in 0..7 {
        if b.p(t).boosts[k] != 0 {
            b.pm(t).boosts[k] = -b.p(t).boosts[k];
            success = true;
        }
    }
    if !success {
        return Val::False;
    }
    add!(b, "-invertboost", t, "[from] move: Topsy-Turvy");
    Val::Undef
}

fn reversal_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let ratio = ((b.p(u).hp as f64 * 48.0 / b.p(u).maxhp as f64).floor()).max(1.0);
    let bp = if ratio < 2.0 {
        200.0
    } else if ratio < 5.0 {
        150.0
    } else if ratio < 10.0 {
        100.0
    } else if ratio < 17.0 {
        80.0
    } else if ratio < 33.0 {
        40.0
    } else {
        20.0
    };
    Val::Num(bp)
}
fn electroball_bp(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(t) = src(a) else { return Val::Num(40.0) };
    let us = b.get_stat(u, 5, false, false);
    let ts = b.get_stat(t, 5, false, false);
    let mut ratio = (us / ts).floor();
    if !ratio.is_finite() {
        ratio = 0.0;
    }
    const BP: [f64; 5] = [40.0, 60.0, 80.0, 120.0, 150.0];
    Val::Num(BP[(ratio as usize).min(4)])
}
fn swallow_try(b: &mut Battle, a: &HArgs) -> Val {
    // onTry(source, target, move)
    let u = tgt(a);
    if b.mv(this_move(a)).source_effect.map(|e| b.eff(e).id == "snatch").unwrap_or(false) {
        return Val::Undef;
    }
    Val::from_bool(b.has_vol(u, "stockpile"))
}
fn swallow_hit(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let sp = b.cond("stockpile");
    let layers = if b.p(u).volatiles.iter().any(|x| x.0 == sp) {
        let l = b.sget(StateLoc::Volatile(u, sp), K::Layers).num();
        if l >= 1.0 { l as usize } else { 1 }
    } else {
        1
    };
    const HEAL: [f64; 3] = [0.25, 0.5, 1.0];
    let amount = b.modify(b.p(u).maxhp as f64, HEAL[(layers - 1).min(2)]);
    let success = b.heal(amount, Some(u), None, EffRef::NONE).truthy();
    if !success {
        add!(b, "-fail", u, "heal");
    }
    b.remove_volatile(u, sp);
    if success { Val::True } else { Val::Empty }
}
fn dive_try_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    if !b.has_vol(u, "dive") && b.has_ability(u, "gulpmissile")
        && b.d.species[b.p(u).species as usize].name == "Cramorant" && !b.p(u).transformed {
        let forme = if b.p(u).hp as f64 <= b.p(u).maxhp as f64 / 2.0 { "Cramorant-Gorging" } else { "Cramorant-Gulping" };
        if let Some(fid) = b.d.species_id(forme) {
            let mr = EffRef { eff: b.mv(this_move(a)).eff, mv: this_move(a), part: 0 };
            b.forme_change(u, fid as u16, mr, false, None);
        }
    }
    charge_turn(b, a, None, false, false)
}
