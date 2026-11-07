//! Condition handlers: statuses, volatiles, side / slot / field conditions, weather, terrain
//! (data/conditions.ts, the Champions mod's conditions, and moves' `condition` blocks).

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
        ("curse", "onStart") => curse_start,
        ("curse", "onResidual") => curse_residual,
        ("safeguard", "durationCallback") => safeguard_duration,
        ("safeguard", "onSetStatus") => safeguard_set_status,
        ("safeguard", "onTryAddVolatile") => safeguard_try_add_volatile,
        ("safeguard", "onSideStart") => safeguard_side_start,
        ("safeguard", "onSideEnd") => safeguard_side_end,
        ("stickyweb", "onSideStart") => stickyweb_side_start,
        ("stickyweb", "onSwitchIn") => stickyweb_switch_in,
        ("syrupbomb", "onStart") => syrupbomb_start,
        ("syrupbomb", "onUpdate") => syrupbomb_update,
        ("syrupbomb", "onResidual") => syrupbomb_residual,
        ("syrupbomb", "onEnd") => syrupbomb_end,
        ("wish", "onStart") => wish_start,
        ("wish", "onResidual") => wish_residual,
        ("wish", "onEnd") => wish_end,
        ("bounce", "onInvulnerability") => bounce_invulnerability,
        ("bounce", "onSourceBasePower") => bounce_source_bp,
        ("noretreat", "onStart") => noretreat_start,
        ("noretreat", "onTrapPokemon") => noretreat_trap,

        ("leechseed", "onStart") => leechseed_start,
        ("leechseed", "onResidual") => leechseed_residual,
        ("destinybond", "onStart") => destinybond_start,
        ("destinybond", "onFaint") => destinybond_faint,
        ("destinybond", "onBeforeMove") => destinybond_before_move,
        ("chillyreception", "onBeforeMove") => chillyreception_before_move,
        ("saltcure", "onStart") => saltcure_start,
        ("saltcure", "onResidual") => saltcure_residual,
        ("saltcure", "onEnd") => saltcure_end,
        ("dive", "onImmunity") => semi_weather_immunity,
        ("dive", "onInvulnerability") => dive_invulnerability,
        ("dive", "onSourceModifyDamage") => dive_source_damage,
        ("dig", "onImmunity") => semi_weather_immunity,
        ("dig", "onInvulnerability") => dig_invulnerability,
        ("dig", "onSourceModifyDamage") => dig_source_damage,
        ("fly", "onInvulnerability") => bounce_invulnerability,
        ("fly", "onSourceBasePower") => bounce_source_bp,
        ("beakblast", "onStart") => beakblast_start,
        ("beakblast", "onHit") => beakblast_hit,
        ("magnetrise", "onStart") => magnetrise_start,
        ("magnetrise", "onImmunity") => magnetrise_immunity,
        ("magnetrise", "onEnd") => magnetrise_end,
        ("powertrick", "onStart") => powertrick_start,
        ("powertrick", "onCopy") => powertrick_swap,
        ("powertrick", "onEnd") => powertrick_end,
        ("powertrick", "onRestart") => powertrick_restart,
        ("electrify", "onStart") => electrify_start,
        ("electrify", "onModifyType") => electrify_modify_type,
        ("aquaring", "onStart") => aquaring_start,
        ("aquaring", "onResidual") => aquaring_residual,
        ("octolock", "onStart") => octolock_start,
        ("octolock", "onResidual") => octolock_residual,
        ("octolock", "onTrapPokemon") => octolock_trap,
        ("uproar", "onStart") => uproar_start,
        ("uproar", "onResidual") => uproar_residual,
        ("uproar", "onEnd") => uproar_end,
        ("uproar", "onAnySetStatus") => uproar_any_set_status,
        ("futuremove", "onStart") => futuremove_start,
        ("futuremove", "onResidual") => futuremove_residual,
        ("futuremove", "onEnd") => futuremove_end,
        ("fling", "onUpdate") => fling_update,
        ("focuspunch", "onStart") => focuspunch_start,
        ("focuspunch", "onHit") => focuspunch_hit,
        ("focuspunch", "onTryAddVolatile") => focuspunch_try_add_volatile,
        ("mirrorcoat", "onStart") => reflect_cond_start,
        ("mirrorcoat", "onRedirectTarget") => mirrorcoat_redirect,
        ("mirrorcoat", "onDamagingHit") => mirrorcoat_hit,
        ("counter", "onStart") => reflect_cond_start,
        ("counter", "onRedirectTarget") => counter_redirect,
        ("counter", "onDamagingHit") => counter_hit,
        ("destinybond", "onMoveAborted") => destinybond_aborted,
        ("stealthrock", "onSideStart") => stealthrock_side_start,
        ("stealthrock", "onSwitchIn") => stealthrock_switch_in,
        ("spikes", "onSideStart") => spikes_side_start,
        ("spikes", "onSideRestart") => spikes_side_restart,
        ("spikes", "onSwitchIn") => spikes_switch_in,

        // ---- statuses
        ("brn", "onStart") => status_start,
        ("brn", "onResidual") => brn_residual,
        ("par", "onStart") => status_start,
        ("par", "onModifySpe") => par_modify_spe,
        ("par", "onBeforeMove") => par_before_move,
        ("slp", "onStart") => slp_start,
        ("slp", "onBeforeMove") => slp_before_move,
        ("frz", "onStart") => frz_start,
        ("frz", "onBeforeMove") => frz_before_move,
        ("frz", "onModifyMove") => frz_modify_move,
        ("frz", "onAfterMoveSecondary") => frz_after_move_secondary,
        ("frz", "onDamagingHit") => frz_damaging_hit,
        ("psn", "onStart") => status_start,
        ("psn", "onResidual") => psn_residual,
        ("tox", "onStart") => tox_start,
        ("tox", "onSwitchIn") => tox_switch_in,
        ("tox", "onResidual") => tox_residual,
        // ---- volatiles
        ("confusion", "onStart") => confusion_start,
        ("confusion", "onEnd") => confusion_end,
        ("confusion", "onBeforeMove") => confusion_before_move,
        ("flinch", "onBeforeMove") => flinch_before_move,
        ("trapped", "onTrapPokemon") => trapped_trap,
        ("trapped", "onStart") => trapped_start,
        ("partiallytrapped", "durationCallback") => pt_duration,
        ("partiallytrapped", "onStart") => pt_start,
        ("partiallytrapped", "onResidual") => pt_residual,
        ("partiallytrapped", "onEnd") => pt_end,
        ("partiallytrapped", "onTrapPokemon") => pt_trap,
        ("lockedmove", "onResidual") => lockedmove_residual,
        ("lockedmove", "onStart") => lockedmove_start,
        ("lockedmove", "onRestart") => lockedmove_restart,
        ("lockedmove", "onAfterMove") => lockedmove_after_move,
        ("lockedmove", "onEnd") => lockedmove_end,
        ("lockedmove", "onLockMove") => lockedmove_lock,
        ("twoturnmove", "onStart") => twoturn_start,
        ("twoturnmove", "onEnd") => twoturn_end,
        ("twoturnmove", "onLockMove") => twoturn_lock,
        ("twoturnmove", "onMoveAborted") => twoturn_aborted,
        ("choicelock", "onStart") => choicelock_start,
        ("choicelock", "onBeforeMove") => choicelock_before_move,
        ("choicelock", "onDisableMove") => choicelock_disable,
        ("mustrecharge", "onBeforeMove") => mustrecharge_before_move,
        ("mustrecharge", "onStart") => mustrecharge_start,
        ("stall", "onStart") => stall_start,
        ("stall", "onStallMove") => stall_stall_move,
        ("stall", "onRestart") => stall_restart,
        ("allyswitch", "onStart") => stall_start,
        ("allyswitch", "onRestart") => allyswitch_restart,
        ("gem", "onBasePower") => gem_base_power,
        ("commanded", "onStart") => commanded_start,
        ("commanded", "onDragOut") => ret_false,
        ("commanded", "onTrapPokemon") => set_trapped,
        ("commanding", "onDragOut") => ret_false,
        ("commanding", "onTrapPokemon") => set_trapped,
        ("commanding", "onBeforeTurn") => commanding_before_turn,
        ("protect", "onStart") => protect_start,
        ("protect", "onTryHit") => protect_try_hit,
        ("banefulbunker", "onStart") => protect_like_start,
        ("banefulbunker", "onTryHit") => banefulbunker_try_hit,
        ("banefulbunker", "onHit") => noop,
        ("spikyshield", "onStart") => protect_like_start,
        ("spikyshield", "onTryHit") => spikyshield_try_hit,
        ("spikyshield", "onHit") => noop,
        ("kingsshield", "onStart") => protect_start,
        ("kingsshield", "onTryHit") => kingsshield_try_hit,
        ("kingsshield", "onHit") => noop,
        ("quickguard", "onSideStart") => quickguard_side_start,
        ("quickguard", "onTryHit") => quickguard_try_hit,
        ("wideguard", "onSideStart") => wideguard_side_start,
        ("wideguard", "onTryHit") => wideguard_try_hit,
        ("endure", "onStart") => endure_start,
        ("endure", "onDamage") => endure_damage,
        ("followme", "onStart") => followme_start,
        ("followme", "onFoeRedirectTarget") => followme_redirect,
        ("ragepowder", "onStart") => ragepowder_start,
        ("ragepowder", "onFoeRedirectTarget") => ragepowder_redirect,
        ("helpinghand", "onStart") => helpinghand_start,
        ("helpinghand", "onRestart") => helpinghand_restart,
        ("helpinghand", "onBasePower") => helpinghand_base_power,
        ("substitute", "onStart") => substitute_start,
        ("substitute", "onTryPrimaryHit") => substitute_try_primary_hit,
        ("substitute", "onEnd") => substitute_end,
        ("encore", "onStart") => encore_start,
        ("encore", "onResidual") => encore_residual,
        ("encore", "onEnd") => encore_end,
        ("encore", "onDisableMove") => encore_disable,
        ("taunt", "onStart") => taunt_start,
        ("taunt", "onEnd") => taunt_end,
        ("taunt", "onDisableMove") => taunt_disable,
        ("taunt", "onBeforeMove") => taunt_before_move,
        ("disable", "onStart") => disable_start,
        ("disable", "onEnd") => disable_end,
        ("disable", "onBeforeMove") => disable_before_move,
        ("disable", "onDisableMove") => disable_disable,
        ("throatchop", "onStart") => throatchop_start,
        ("throatchop", "onDisableMove") => throatchop_disable,
        ("throatchop", "onBeforeMove") => throatchop_before_move,
        ("throatchop", "onModifyMove") => throatchop_before_move,
        ("throatchop", "onEnd") => throatchop_end,
        ("torment", "onStart") => torment_start,
        ("torment", "onEnd") => torment_end,
        ("torment", "onDisableMove") => torment_disable,
        ("healblock", "durationCallback") => healblock_duration,
        ("healblock", "onStart") => healblock_start,
        ("healblock", "onDisableMove") => healblock_disable,
        ("healblock", "onBeforeMove") => healblock_before_move,
        ("healblock", "onModifyMove") => healblock_before_move,
        ("healblock", "onEnd") => healblock_end,
        ("healblock", "onTryHeal") => healblock_try_heal,
        ("healblock", "onRestart") => healblock_restart,
        ("yawn", "onStart") => yawn_start,
        ("yawn", "onEnd") => yawn_end,
        ("perishsong", "onEnd") => perishsong_end,
        ("perishsong", "onResidual") => perishsong_residual,
        ("roost", "onStart") => roost_start,
        ("roost", "onType") => roost_type,
        ("smackdown", "onStart") => smackdown_start,
        ("smackdown", "onRestart") => smackdown_restart,
        ("focusenergy", "onStart") => focusenergy_start,
        ("focusenergy", "onModifyCritRatio") => focusenergy_crit,
        ("dragoncheer", "onStart") => dragoncheer_start,
        ("dragoncheer", "onModifyCritRatio") => dragoncheer_crit,
        ("glaiverush", "onStart") => glaiverush_start,
        ("glaiverush", "onAccuracy") => ret_true,
        ("glaiverush", "onSourceModifyDamage") => chain2,
        ("glaiverush", "onBeforeMove") => glaiverush_before_move,
        ("minimize", "onRestart") => ret_null,
        ("minimize", "onSourceModifyDamage") => minimize_damage,
        ("minimize", "onAccuracy") => minimize_accuracy,
        ("stockpile", "onStart") => stockpile_start,
        ("stockpile", "onRestart") => stockpile_restart,
        ("stockpile", "onEnd") => stockpile_end,
        ("imprison", "onStart") => imprison_start,
        ("imprison", "onFoeDisableMove") => imprison_disable,
        ("imprison", "onFoeBeforeMove") => imprison_before_move,
        ("flashfire", "onStart") => flashfire_start,
        ("flashfire", "onModifyAtk") => flashfire_mod,
        ("flashfire", "onModifySpA") => flashfire_mod,
        ("flashfire", "onEnd") => flashfire_end,
        ("unburden", "onModifySpe") => unburden_spe,
        ("charge", "onStart") => charge_start,
        ("charge", "onRestart") => charge_start,
        ("charge", "onBasePower") => charge_base_power,
        ("charge", "onMoveAborted") => charge_after,
        ("charge", "onAfterMove") => charge_after,
        ("charge", "onEnd") => charge_end,
        ("attract", "onStart") => attract_start,
        ("attract", "onUpdate") => attract_update,
        ("attract", "onBeforeMove") => attract_before_move,
        ("attract", "onEnd") => attract_end,
        ("healingwish", "onSwitchIn") => healingwish_switch_in,
        ("healingwish", "onSwap") => healingwish_swap,
        // ---- side conditions
        ("reflect", "durationCallback") => screen_duration,
        ("reflect", "onAnyModifyDamage") => reflect_damage,
        ("reflect", "onSideStart") => reflect_side_start,
        ("reflect", "onSideEnd") => reflect_side_end,
        ("lightscreen", "durationCallback") => screen_duration,
        ("lightscreen", "onAnyModifyDamage") => lightscreen_damage,
        ("lightscreen", "onSideStart") => lightscreen_side_start,
        ("lightscreen", "onSideEnd") => lightscreen_side_end,
        ("auroraveil", "durationCallback") => screen_duration,
        ("auroraveil", "onAnyModifyDamage") => auroraveil_damage,
        ("auroraveil", "onSideStart") => auroraveil_side_start,
        ("auroraveil", "onSideEnd") => auroraveil_side_end,
        ("tailwind", "durationCallback") => tailwind_duration,
        ("tailwind", "onSideStart") => tailwind_side_start,
        ("tailwind", "onModifySpe") => chain2,
        ("tailwind", "onSideEnd") => tailwind_side_end,
        ("toxicspikes", "onSideStart") => toxicspikes_side_start,
        ("toxicspikes", "onSideRestart") => toxicspikes_side_restart,
        ("toxicspikes", "onSwitchIn") => toxicspikes_switch_in,
        // ---- field
        ("trickroom", "durationCallback") => trickroom_duration,
        ("trickroom", "onFieldStart") => trickroom_field_start,
        ("trickroom", "onFieldRestart") => trickroom_field_restart,
        ("trickroom", "onFieldEnd") => trickroom_field_end,
        ("gravity", "durationCallback") => gravity_duration,
        ("gravity", "onFieldStart") => gravity_field_start,
        ("gravity", "onModifyAccuracy") => gravity_accuracy,
        ("gravity", "onDisableMove") => gravity_disable,
        ("gravity", "onBeforeMove") => gravity_before_move,
        ("gravity", "onModifyMove") => gravity_before_move,
        ("gravity", "onFieldEnd") => gravity_field_end,
        ("raindance", "durationCallback") => rain_duration,
        ("raindance", "onWeatherModifyDamage") => rain_damage,
        ("raindance", "onFieldStart") => weather_field_start,
        ("raindance", "onFieldResidual") => weather_field_residual,
        ("raindance", "onFieldEnd") => weather_field_end,
        ("sunnyday", "durationCallback") => sun_duration,
        ("sunnyday", "onWeatherModifyDamage") => sun_damage,
        ("sunnyday", "onFieldStart") => weather_field_start,
        ("sunnyday", "onImmunity") => sun_immunity,
        ("sunnyday", "onFieldResidual") => weather_field_residual,
        ("sunnyday", "onFieldEnd") => weather_field_end,
        ("sandstorm", "durationCallback") => sand_duration,
        ("sandstorm", "onModifySpD") => sand_spd,
        ("sandstorm", "onFieldStart") => weather_field_start,
        ("sandstorm", "onFieldResidual") => weather_field_residual,
        ("sandstorm", "onWeather") => weather_chip,
        ("sandstorm", "onFieldEnd") => weather_field_end,
        ("snowscape", "durationCallback") => snow_duration,
        ("snowscape", "onModifyDef") => snow_def,
        ("snowscape", "onFieldStart") => weather_field_start,
        ("snowscape", "onFieldResidual") => weather_field_residual,
        ("snowscape", "onFieldEnd") => weather_field_end,
        ("hail", "durationCallback") => snow_duration,
        ("hail", "onFieldStart") => weather_field_start,
        ("hail", "onFieldResidual") => weather_field_residual,
        ("hail", "onWeather") => weather_chip,
        ("hail", "onFieldEnd") => weather_field_end,
        ("psychicterrain", "durationCallback") => terrain_duration,
        ("psychicterrain", "onTryHit") => psychicterrain_try_hit,
        ("psychicterrain", "onBasePower") => psychicterrain_base_power,
        ("psychicterrain", "onFieldStart") => terrain_field_start,
        ("psychicterrain", "onFieldEnd") => terrain_field_end,
        ("grassyterrain", "durationCallback") => terrain_duration,
        ("grassyterrain", "onBasePower") => grassyterrain_base_power,
        ("grassyterrain", "onFieldStart") => terrain_field_start,
        ("grassyterrain", "onResidual") => grassyterrain_residual,
        ("grassyterrain", "onFieldEnd") => terrain_field_end,
        ("electricterrain", "durationCallback") => terrain_duration,
        ("electricterrain", "onSetStatus") => electricterrain_set_status,
        ("electricterrain", "onTryAddVolatile") => electricterrain_try_add_volatile,
        ("electricterrain", "onBasePower") => electricterrain_base_power,
        ("electricterrain", "onFieldStart") => terrain_field_start,
        ("electricterrain", "onFieldEnd") => terrain_field_end,
        ("mistyterrain", "durationCallback") => terrain_duration,
        ("mistyterrain", "onSetStatus") => mistyterrain_set_status,
        ("mistyterrain", "onTryAddVolatile") => mistyterrain_try_add_volatile,
        ("mistyterrain", "onBasePower") => mistyterrain_base_power,
        ("mistyterrain", "onFieldStart") => terrain_field_start,
        ("mistyterrain", "onFieldEnd") => terrain_field_end,
        _ => return None,
    };
    Some(f)
}

// ------------------------------------------------------------------
// small shared handlers
// ------------------------------------------------------------------
fn noop(_: &mut Battle, _: &HArgs) -> Val {
    Val::Undef
}
fn ret_false(_: &mut Battle, _: &HArgs) -> Val {
    Val::False
}
fn ret_true(_: &mut Battle, _: &HArgs) -> Val {
    Val::True
}
fn ret_null(_: &mut Battle, _: &HArgs) -> Val {
    Val::Null
}
fn chain2(b: &mut Battle, _: &HArgs) -> Val {
    b.chain_modify(2.0, 1.0);
    Val::Undef
}
fn set_trapped(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).trapped = 1;
    Val::Undef
}
fn ability_from(b: &Battle, a: &HArgs) -> Option<(String, String)> {
    if b.etype(a.effect) == EffType::Ability {
        let n = b.ename(a.effect).to_string();
        let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
        Some((n, s))
    } else {
        None
    }
}

// ------------------------------------------------------------------
// statuses
// ------------------------------------------------------------------
fn status_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let sid = b.eff(a.this_eff).id.clone();
    if sid == "brn" && b.eid(a.effect) == "flameorb" {
        add!(b, "-status", t, "brn", "[from] item: Flame Orb");
    } else if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-status", t, sid, format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-status", t, sid);
    }
    Val::Undef
}
fn brn_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = b.p(t).base_maxhp as f64 / 16.0;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn psn_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = b.p(t).base_maxhp as f64 / 8.0;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn tox_start(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Stage, Val::Num(0.0));
    let t = tgt(a);
    if b.eid(a.effect) == "toxicorb" {
        add!(b, "-status", t, "tox", "[from] item: Toxic Orb");
    } else if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-status", t, "tox", format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-status", t, "tox");
    }
    Val::Undef
}
fn tox_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Stage, Val::Num(0.0));
    Val::Undef
}
fn tox_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let mut stage = b.sget(a.state, K::Stage).num();
    if stage < 15.0 {
        stage += 1.0;
        b.sset(a.state, K::Stage, Val::Num(stage));
    }
    let d = clamp_int(b.p(t).base_maxhp as f64 / 16.0, Some(1.0), None) * stage;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn par_modify_spe(b: &mut Battle, a: &HArgs) -> Val {
    let mut spe = b.final_modify(a.relay.num());
    let t = tgt(a);
    if !b.has_ability(t, "quickfeet") {
        spe = (spe * 50.0 / 100.0).floor();
    }
    Val::Num(spe)
}
fn par_before_move(b: &mut Battle, a: &HArgs) -> Val {
    if b.random_chance(1, 8) {
        let t = tgt(a);
        add!(b, "cant", t, "par");
        return Val::False;
    }
    Val::Undef
}
fn slp_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let et = b.etype(a.effect);
    if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-status", t, "slp", format!("[from] ability: {n}"), format!("[of] {s}"));
    } else if et == EffType::Move {
        let n = b.ename(a.effect).to_string();
        add!(b, "-status", t, "slp", format!("[from] move: {n}"));
    } else {
        add!(b, "-status", t, "slp");
    }
    let table = [2.0, 3.0, 3.0];
    let i = b.prng.sample_index(3);
    b.sset(a.state, K::StartTime, Val::Num(table[i]));
    b.sset(a.state, K::Time, Val::Num(table[i]));
    if b.remove_volatile_id(t, "nightmare") {
        add!(b, "-end", t, "Nightmare", "[silent]");
    }
    Val::Undef
}
fn slp_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let loc = StateLoc::Status(m);
    let mut time = b.sget(loc, K::Time).num();
    if b.has_ability(m, "earlybird") {
        time -= 1.0;
    }
    time -= 1.0;
    b.sset(loc, K::Time, Val::Num(time));
    if time <= 0.0 {
        b.cure_status(m, false);
        return Val::Undef;
    }
    add!(b, "cant", m, "slp");
    if let Some(mv) = amv(a) {
        if b.mv(mv).sleep_usable {
            return Val::Undef;
        }
    }
    Val::False
}
fn frz_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-status", t, "frz", format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-status", t, "frz");
    }
    b.sset(a.state, K::StartTime, Val::Num(3.0));
    b.sset(a.state, K::Time, Val::Num(3.0));
    Val::Undef
}
fn frz_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::DEFROST != 0 && !(b.move_def(mv).id == "burnup" && !b.has_type(m, "Fire")) {
        return Val::Undef;
    }
    let loc = StateLoc::Status(m);
    let time = b.sget(loc, K::Time).num() - 1.0;
    b.sset(loc, K::Time, Val::Num(time));
    if time <= 0.0 || b.random_chance(1, 4) {
        b.cure_status(m, false);
        return Val::Undef;
    }
    add!(b, "cant", m, "frz");
    Val::False
}
fn frz_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::DEFROST != 0 {
        let n = b.move_def(mv).name.clone();
        add!(b, "-curestatus", m, "frz", format!("[from] move: {n}"));
        b.clear_status(m);
    }
    Val::Undef
}
fn frz_after_move_secondary(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).thaws_target {
            b.cure_status(tgt(a), false);
        }
    }
    Val::Undef
}
fn frz_damaging_hit(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        let fire = b.d.type_id("Fire");
        if b.mv(mv).type_ == fire && b.mv(mv).category != Category::Status && b.move_def(mv).id != "polarflare" {
            b.cure_status(tgt(a), false);
        }
    }
    Val::Undef
}

// ------------------------------------------------------------------
// volatiles
// ------------------------------------------------------------------
fn confusion_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.eid(a.effect) == "lockedmove" {
        add!(b, "-start", t, "confusion", "[fatigue]");
    } else if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-start", t, "confusion", format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-start", t, "confusion");
    }
    let min = if b.eid(a.effect) == "axekick" { 3 } else { 2 };
    let time = b.random_range(min, 6);
    b.sset(a.state, K::Time, Val::Num(time as f64));
    Val::Undef
}
fn confusion_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "confusion");
    Val::Undef
}
fn confusion_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let c = b.d.c.confusion;
    let loc = StateLoc::Volatile(m, c);
    let time = b.sget(loc, K::Time).num() - 1.0;
    b.sset(loc, K::Time, Val::Num(time));
    if time == 0.0 {
        b.remove_volatile(m, c);
        return Val::Undef;
    }
    add!(b, "-activate", m, "confusion");
    if !b.random_chance(33, 100) {
        return Val::Undef;
    }
    b.active_target = Some(m);
    let dmg = b.get_confusion_damage(m, 40.0);
    let confused = b.d.cond("confused").unwrap();
    b.damage(dmg, Some(m), Some(m), EffRef::eff(confused), false);
    Val::False
}
fn flinch_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "cant", m, "flinch");
    b.run_event(Ev::Flinch, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
    Val::False
}
fn trapped_trap(b: &mut Battle, a: &HArgs) -> Val {
    b.try_trap(tgt(a), false);
    Val::Undef
}
fn trapped_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-activate", t, "trapped");
    Val::Undef
}
fn pt_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        if b.has_item(s, "gripclaw") {
            return Val::Num(8.0);
        }
    }
    Val::Num(b.random_range(5, 7) as f64)
}
fn pt_source_effect(b: &Battle, loc: StateLoc) -> (String, String) {
    let se = b.state(loc).and_then(|s| s.source_effect);
    match se {
        Some(e) => (b.eff(e).name.clone(), b.eff(e).id.clone()),
        None => (String::new(), String::new()),
    }
}
fn pt_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let (n, _) = pt_source_effect(b, a.state);
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-activate", t, format!("move: {n}"), format!("[of] {s}"));
    let bd = if src(a).map(|s| b.has_item(s, "bindingband")).unwrap_or(false) { 6.0 } else { 8.0 };
    b.sset(a.state, K::BoundDivisor, Val::Num(bd));
    Val::Undef
}
fn pt_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let source = b.state_source(a.state);
    let (n, id) = pt_source_effect(b, a.state);
    let gmax = id == "gmaxcentiferno" || id == "gmaxsandblast";
    if let Some(s) = source {
        if (!b.p(s).is_active || b.p(s).hp <= 0 || b.p(s).active_turns == 0) && !gmax {
            b.delete_volatile(m, "partiallytrapped");
            add!(b, "-end", m, n, "[partiallytrapped]", "[silent]");
            return Val::Undef;
        }
    }
    let bd = b.sget(a.state, K::BoundDivisor).num();
    let d = b.p(m).base_maxhp as f64 / bd;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn pt_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let (n, _) = pt_source_effect(b, a.state);
    add!(b, "-end", m, n, "[partiallytrapped]");
    Val::Undef
}
fn pt_trap(b: &mut Battle, a: &HArgs) -> Val {
    let (_, id) = pt_source_effect(b, a.state);
    let gmax = id == "gmaxcentiferno" || id == "gmaxsandblast";
    let src_active = b.state_source(a.state).map(|s| b.p(s).is_active).unwrap_or(false);
    if src_active || gmax {
        b.try_trap(tgt(a), false);
    }
    Val::Undef
}
fn lockedmove_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.slp {
        b.delete_volatile(m, "lockedmove");
    }
    let td = b.sget(a.state, K::TrueDuration).num() - 1.0;
    b.sset(a.state, K::TrueDuration, Val::Num(td));
    Val::Undef
}
fn lockedmove_start(b: &mut Battle, a: &HArgs) -> Val {
    let td = b.random_range(2, 4);
    b.sset(a.state, K::TrueDuration, Val::Num(td as f64));
    // effectState.move = effect.id (the move that locked)
    let mid = if a.effect.is_move() { b.mv(a.effect.mv).def } else { b.eff(a.effect.eff).move_def };
    b.sset(a.state, K::Move, Val::MoveId(mid));
    Val::Undef
}
fn lockedmove_restart(b: &mut Battle, a: &HArgs) -> Val {
    if b.sget(a.state, K::TrueDuration).num() >= 2.0 {
        b.set_state_duration(a.state, Some(2.0));
    }
    Val::Undef
}
fn lockedmove_after_move(b: &mut Battle, a: &HArgs) -> Val {
    if b.state_duration(a.state) == Some(1.0) {
        let lm = b.d.c.lockedmove;
        b.remove_volatile(tgt(a), lm);
    }
    Val::Undef
}
fn lockedmove_end(b: &mut Battle, a: &HArgs) -> Val {
    if b.sget(a.state, K::TrueDuration).num() > 1.0 {
        return Val::Undef;
    }
    let c = b.d.c.confusion;
    b.add_volatile(tgt(a), c, None, EffRef::NONE, None);
    Val::Undef
}
fn lockedmove_lock(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_vol(m, "dynamax") {
        return Val::Undef;
    }
    b.sget(a.state, K::Move)
}
fn twoturn_start(b: &mut Battle, a: &HArgs) -> Val {
    let attacker = tgt(a);
    let defender = src(a);
    // effect: the move (an active move)
    let mv = amv(a);
    let mid = match mv {
        Some(x) => b.mv(x).def,
        None => b.eff(a.effect.eff).move_def,
    };
    b.sset(a.state, K::Move, Val::MoveId(mid));
    let mid_str = b.d.moves[mid as usize].id.clone();
    if let Some(c) = b.d.cond(&mid_str) {
        b.add_volatile(attacker, c, None, EffRef::NONE, None);
    }
    let mut loc = b.p(attacker).last_move_target_loc.unwrap_or(0);
    let has_source_effect = mv.map(|x| b.mv(x).source_effect.is_some()).unwrap_or(false);
    let mut def = defender;
    if has_source_effect && b.d.moves[mid as usize].target != MoveTarget::SelfTarget {
        if let Some(dd) = def {
            if b.p(dd).fainted {
                let foes = b.allies_and_self_all(1 - attacker.s);
                let i = b.prng.sample_index(foes.len());
                def = Some(foes[i]);
            }
        }
        if let Some(dd) = def {
            loc = b.get_loc_of(attacker, dd);
        }
    }
    if let Some(c) = b.d.cond(&mid_str) {
        if let Some(v) = b.volatile_mut(attacker, c) {
            v.set(K::TargetLoc, Val::Num(loc as f64));
        }
    }
    b.attr_last_move(&["[still]"]);
    b.run_event(Ev::PrepareHit, Arg::Mon(attacker), def, a.effect, Val::Undef);
    Val::Undef
}
fn twoturn_end(b: &mut Battle, a: &HArgs) -> Val {
    if let Val::MoveId(mid) = b.sget(a.state, K::Move) {
        let id = b.d.moves[mid as usize].id.clone();
        b.remove_volatile_id(tgt(a), &id);
    }
    Val::Undef
}
fn twoturn_lock(b: &mut Battle, a: &HArgs) -> Val {
    b.sget(a.state, K::Move)
}
fn twoturn_aborted(b: &mut Battle, a: &HArgs) -> Val {
    let c = b.d.c.twoturnmove;
    b.remove_volatile(tgt(a), c);
    Val::Undef
}
fn choicelock_start(b: &mut Battle, a: &HArgs) -> Val {
    let Some(am) = b.active_move else { return Val::Undef };
    if b.mv(am).has_bounced || b.mv(am).source_effect.map(|e| b.eff(e).id == "snatch").unwrap_or(false) {
        return Val::False;
    }
    let mid = b.mv(am).def;
    b.sset(a.state, K::Move, Val::MoveId(mid));
    Val::Undef
}
fn choicelock_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let is_choice = b.eff(b.p(m).item).item.as_ref().map(|i| i.is_choice).unwrap_or(false);
    if !is_choice {
        let c = b.d.c.choicelock;
        b.remove_volatile(m, c);
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    let locked = b.sget(a.state, K::Move);
    let mid = b.mv(mv).def;
    if !b.ignoring_item(m) && !b.has_vol(m, "dynamax") && locked != Val::MoveId(mid) && b.move_def(mv).id != "struggle" {
        if b.log.is_some() {
            let ms = b.mon_str(m);
            let n = b.move_def(mv).name.clone();
            b.add_move(vec!["move".into(), ms, n]);
        }
        b.attr_last_move(&["[still]"]);
        add!(b, "-fail", m);
        return Val::False;
    }
    Val::Undef
}
fn choicelock_disable(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let is_choice = b.eff(b.p(m).item).item.as_ref().map(|i| i.is_choice).unwrap_or(false);
    let locked = match b.sget(a.state, K::Move) {
        Val::MoveId(x) => Some(x),
        _ => None,
    };
    if !is_choice || !locked.map(|l| b.has_move(m, l)).unwrap_or(false) {
        let c = b.d.c.choicelock;
        b.remove_volatile(m, c);
        return Val::Undef;
    }
    if b.ignoring_item(m) || b.has_vol(m, "dynamax") {
        return Val::Undef;
    }
    let src_name = b.state(a.state).and_then(|s| s.source_effect).map(|e| b.eff(e).name.clone());
    let ids: Vec<u16> = b.p(m).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        if Some(id) != locked {
            b.disable_move(m, id, false, src_name.clone());
        }
    }
    Val::Undef
}
fn mustrecharge_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "cant", m, "recharge");
    let c = b.d.c.mustrecharge;
    b.remove_volatile(m, c);
    b.remove_volatile_id(m, "truant");
    Val::Null
}
fn mustrecharge_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-mustrecharge", m);
    Val::Undef
}
fn stall_start(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Counter, Val::Num(3.0));
    Val::Undef
}
fn stall_stall_move(b: &mut Battle, a: &HArgs) -> Val {
    let c = b.sget(a.state, K::Counter).num();
    let counter = if c == 0.0 { 1.0 } else { c };
    let ok = b.random_chance(1, counter as u32);
    if !ok {
        let st = b.d.c.stall;
        let m = tgt(a);
        b.pm(m).volatiles.retain(|x| x.0 != st);
    }
    Val::from_bool(ok)
}
fn stall_restart(b: &mut Battle, a: &HArgs) -> Val {
    let c = b.sget(a.state, K::Counter).num();
    let max = b.eff(a.this_eff).counter_max as f64;
    if c < max {
        b.sset(a.state, K::Counter, Val::Num(c * 3.0));
    }
    b.set_state_duration(a.state, Some(2.0));
    Val::Undef
}
fn allyswitch_restart(b: &mut Battle, a: &HArgs) -> Val {
    let c = b.sget(a.state, K::Counter).num();
    let counter = if c == 0.0 { 1.0 } else { c };
    let ok = b.random_chance(1, counter as u32);
    if !ok {
        b.delete_volatile(tgt(a), "allyswitch");
        return Val::False;
    }
    let max = b.eff(a.this_eff).counter_max as f64;
    if c < max {
        b.sset(a.state, K::Counter, Val::Num(c * 3.0));
    }
    b.set_state_duration(a.state, Some(2.0));
    Val::Undef
}
fn gem_base_power(b: &mut Battle, _: &HArgs) -> Val {
    b.chain_modify(5325.0, 4096.0);
    Val::Undef
}
fn commanded_start(b: &mut Battle, a: &HArgs) -> Val {
    let mut bo = SparseBoosts::default();
    for (s, v) in [(B_ATK, 2), (B_SPA, 2), (B_SPE, 2), (B_DEF, 2), (B_SPD, 2)] {
        bo.set(s, v);
    }
    b.boost(bo, Some(tgt(a)), None, EffRef::NONE, false, false);
    Val::Undef
}
fn commanding_before_turn(b: &mut Battle, a: &HArgs) -> Val {
    b.cancel_action(tgt(a));
    Val::Undef
}

// ---- Protect & co
fn protect_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "Protect");
    Val::Undef
}
fn protect_like_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "move: Protect");
    Val::Undef
}
/// the common part of the Protect-like onTryHit; returns false if the move goes through
fn protect_block(b: &mut Battle, a: &HArgs, msg: &str) -> Option<(Mon, Mon, u16)> {
    let target = tgt(a);
    let source = src(a)?;
    let mv = amv(a)?;
    if b.check_move_bypasses_protect(mv, source, target) {
        return None;
    }
    if b.mv(mv).smart_target == Some(true) {
        b.mvm(mv).smart_target = Some(false);
    } else {
        add!(b, "-activate", target, msg);
    }
    let lm = b.d.c.lockedmove;
    if b.has_volatile(source, lm) && b.volatile(source, lm).and_then(|v| v.duration) == Some(2.0) {
        b.pm(source).volatiles.retain(|x| x.0 != lm);
    }
    Some((target, source, mv))
}
fn protect_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    match protect_block(b, a, "move: Protect") {
        Some(_) => Val::Empty,
        None => Val::Undef,
    }
}
fn banefulbunker_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some((target, source, mv)) = protect_block(b, a, "move: Protect") else { return Val::Undef };
    if b.check_move_makes_contact(mv, source, target, false) {
        let bb = b.d.move_id("banefulbunker").unwrap();
        let am = b.new_active_move(bb);
        let mr = EffRef { eff: b.mv(am).eff, mv: am, part: 0 };
        let psn = b.d.c.psn;
        b.try_set_status(source, psn, Some(target), mr);
    }
    Val::Empty
}
fn spikyshield_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some((target, source, mv)) = protect_block(b, a, "move: Protect") else { return Val::Undef };
    if b.check_move_makes_contact(mv, source, target, false) {
        let d = b.p(source).base_maxhp as f64 / 8.0;
        b.damage(d, Some(source), Some(target), EffRef::NONE, false);
    }
    Val::Empty
}
fn kingsshield_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_bypasses_protect_bs(mv, source, target, false) {
        return Val::Undef;
    }
    if b.mv(mv).smart_target == Some(true) {
        b.mvm(mv).smart_target = Some(false);
    } else {
        add!(b, "-activate", target, "move: Protect");
    }
    let lm = b.d.c.lockedmove;
    if b.volatile(source, lm).and_then(|v| v.duration) == Some(2.0) {
        b.pm(source).volatiles.retain(|x| x.0 != lm);
    }
    if b.check_move_makes_contact(mv, source, target, false) {
        let bo = Battle::boosts1(B_ATK, -1);
        let ks = b.d.move_id("kingsshield").unwrap();
        let am = b.new_active_move(ks);
        let mr = EffRef { eff: b.mv(am).eff, mv: am, part: 0 };
        b.boost(bo, Some(source), Some(target), mr, false, false);
    }
    Val::Empty
}
fn quickguard_side_start(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        add!(b, "-singleturn", s, "Quick Guard");
    }
    Val::Undef
}
fn quickguard_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).priority <= 0.1 {
        return Val::Undef;
    }
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    if b.check_move_bypasses_protect(mv, source, target) {
        return Val::Undef;
    }
    add!(b, "-activate", target, "move: Quick Guard");
    let lm = b.d.c.lockedmove;
    if b.volatile(source, lm).and_then(|v| v.duration) == Some(2.0) {
        b.pm(source).volatiles.retain(|x| x.0 != lm);
    }
    Val::Empty
}
fn wideguard_side_start(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        add!(b, "-singleturn", s, "Wide Guard");
    }
    Val::Undef
}
fn wideguard_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if !matches!(b.mv(mv).target, MoveTarget::AllAdjacent | MoveTarget::AllAdjacentFoes) {
        return Val::Undef;
    }
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    if b.check_move_bypasses_protect(mv, source, target) {
        return Val::Undef;
    }
    add!(b, "-activate", target, "move: Wide Guard");
    let lm = b.d.c.lockedmove;
    if b.volatile(source, lm).and_then(|v| v.duration) == Some(2.0) {
        b.pm(source).volatiles.retain(|x| x.0 != lm);
    }
    Val::Empty
}
fn endure_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "move: Endure");
    Val::Undef
}
fn endure_damage(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.etype(a.effect) == EffType::Move && a.relay.num() >= b.p(t).hp as f64 {
        add!(b, "-activate", t, "move: Endure");
        return Val::Num((b.p(t).hp - 1) as f64);
    }
    Val::Undef
}
fn followme_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.eid(a.effect) == "zpower" {
        add!(b, "-singleturn", t, "move: Follow Me", "[zeffect]");
    } else {
        add!(b, "-singleturn", t, "move: Follow Me");
    }
    Val::Undef
}
fn followme_redirect(b: &mut Battle, a: &HArgs) -> Val {
    let user = holder(a);
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if !b.is_sky_dropped(user) && b.valid_target(user, source, b.mv(mv).target) {
        if b.mv(mv).smart_target == Some(true) {
            b.mvm(mv).smart_target = Some(false);
        }
        return Val::Mon(user);
    }
    Val::Undef
}
fn ragepowder_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "move: Rage Powder");
    Val::Undef
}
fn ragepowder_redirect(b: &mut Battle, a: &HArgs) -> Val {
    let user = holder(a);
    if b.is_sky_dropped(user) {
        return Val::Undef;
    }
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.run_status_immunity(source, "powder", false) && b.valid_target(user, source, b.mv(mv).target) {
        if b.mv(mv).smart_target == Some(true) {
            b.mvm(mv).smart_target = Some(false);
        }
        return Val::Mon(user);
    }
    Val::Undef
}
fn helpinghand_start(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Multiplier, Val::Num(1.5));
    let t = tgt(a);
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-singleturn", t, "Helping Hand", format!("[of] {s}"));
    Val::Undef
}
fn helpinghand_restart(b: &mut Battle, a: &HArgs) -> Val {
    let mlt = b.sget(a.state, K::Multiplier).num() * 1.5;
    b.sset(a.state, K::Multiplier, Val::Num(mlt));
    let t = tgt(a);
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-singleturn", t, "Helping Hand", format!("[of] {s}"));
    Val::Undef
}
fn helpinghand_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let mlt = b.sget(a.state, K::Multiplier).num();
    b.chain_modify(mlt, 1.0);
    Val::Undef
}
fn substitute_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.eid(a.effect) == "shedtail" {
        add!(b, "-start", t, "Substitute", "[from] move: Shed Tail");
    } else {
        add!(b, "-start", t, "Substitute");
    }
    let hp = (b.p(t).maxhp as f64 / 4.0).floor();
    b.sset(a.state, K::Hp, Val::Num(hp));
    let pt = b.d.c.partiallytrapped;
    if b.has_volatile(t, pt) {
        let se = b.volatile(t, pt).and_then(|v| v.source_effect).map(|e| b.eff(e).name.clone()).unwrap_or_default();
        add!(b, "-end", t, se, "[partiallytrapped]", "[silent]");
        b.pm(t).volatiles.retain(|x| x.0 != pt);
    }
    Val::Undef
}
fn substitute_try_primary_hit(b: &mut Battle, a: &HArgs) -> Val {
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if target == source || b.mv(mv).flags & mflag::BYPASSSUB != 0 || b.mv(mv).infiltrates {
        return Val::Undef;
    }
    let d = b.get_damage(source, target, mv, false);
    if !d.truthy() && !matches!(d, Val::Num(x) if x == 0.0) {
        add!(b, "-fail", source);
        b.attr_last_move(&["[still]"]);
        return Val::Null;
    }
    let sub = b.d.c.substitute;
    let mut damage = d.num();
    let sub_hp = b.volatile(target, sub).map(|v| v.num(K::Hp)).unwrap_or(0.0);
    if damage > sub_hp {
        damage = sub_hp;
    }
    if let Some(v) = b.volatile_mut(target, sub) {
        v.set(K::Hp, Val::Num(sub_hp - damage));
    }
    b.pm(source).last_damage = damage as i32;
    if sub_hp - damage <= 0.0 {
        if b.mv(mv).ohko {
            add!(b, "-ohko");
        }
        b.remove_volatile(target, sub);
    } else {
        add!(b, "-activate", target, "move: Substitute", "[damage]");
    }
    if damage != 0.0 {
        b.apply_recoil_damage_pub(damage, mv, source);
    }
    if let Some((dn, dd)) = b.mv(mv).drain {
        let drain = b.d.c.drain;
        b.heal((damage * dn / dd).ceil(), Some(source), Some(target), EffRef::eff(drain));
    }
    let mr = EffRef { eff: b.mv(mv).eff, mv, part: 0 };
    b.single_event(Ev::AfterSubDamage, mr, StateLoc::None, Arg::Mon(target), Arg::Mon(source), mr, Val::Num(damage));
    b.run_event(Ev::AfterSubDamage, Arg::Mon(target), Some(source), mr, Val::Num(damage));
    Val::Num(0.0)
}
fn substitute_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Substitute");
    Val::Undef
}
fn encore_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(lm) = b.p(t).last_move else { return Val::False };
    if b.has_vol(t, "dynamax") {
        return Val::False;
    }
    let md = &b.d.moves[lm as usize];
    let slot_pp = b.p(t).move_slots.iter().find(|s| s.id == lm).map(|s| s.pp);
    if md.flags & mflag::FAILENCORE != 0 || slot_pp.is_none() || slot_pp.unwrap() <= 0 {
        return Val::False;
    }
    b.sset(a.state, K::Move, Val::MoveId(lm));
    add!(b, "-start", t, "Encore");
    match b.will_move(t) {
        None => {
            let d = b.state_duration(a.state).unwrap_or(0.0) + 1.0;
            b.set_state_duration(a.state, Some(d));
        }
        Some(qi) => {
            if b.queue[qi].move_id != lm && !b.has_item(t, "mentalherb") {
                // queue.changeAction(target, {choice: 'move', moveid})
                b.cancel_action(t);
                let mut act = Action::new(ActionKind::Move);
                act.pokemon = Some(t);
                act.move_id = lm;
                b.insert_choice(act, false);
            }
        }
    }
    Val::Undef
}
fn encore_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let mv = match b.sget(a.state, K::Move) {
        Val::MoveId(x) => Some(x),
        _ => None,
    };
    let pp = mv.and_then(|m| b.p(t).move_slots.iter().find(|s| s.id == m).map(|s| s.pp));
    if pp.is_none() || pp.unwrap() <= 0 {
        b.remove_volatile_id(t, "encore");
    }
    Val::Undef
}
fn encore_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Encore");
    Val::Undef
}
fn encore_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Val::MoveId(mv) = b.sget(a.state, K::Move) else { return Val::Undef };
    if !b.has_move(t, mv) {
        return Val::Undef;
    }
    let ids: Vec<u16> = b.p(t).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        if id != mv {
            b.disable_move(t, id, false, None);
        }
    }
    Val::Undef
}
fn taunt_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).active_turns != 0 && b.will_move(t).is_none() {
        let d = b.state_duration(a.state).unwrap_or(0.0) + 1.0;
        b.set_state_duration(a.state, Some(d));
    }
    add!(b, "-start", t, "move: Taunt");
    Val::Undef
}
fn taunt_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "move: Taunt");
    Val::Undef
}
fn taunt_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let ids: Vec<u16> = b.p(t).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        let md = &b.d.moves[id as usize];
        if md.category == Category::Status && md.id != "mefirst" {
            b.disable_move(t, id, false, None);
        }
    }
    Val::Undef
}
fn taunt_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Status && b.move_def(mv).id != "mefirst" {
        let t = tgt(a);
        let n = b.move_def(mv).name.clone();
        add!(b, "cant", t, "move: Taunt", n);
        return Val::False;
    }
    Val::Undef
}
fn disable_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let is_active_mover = b.active_pokemon == Some(t) && b.active_move.map(|am| !b.mv(am).is_external).unwrap_or(false);
    if b.will_move(t).is_some() || is_active_mover {
        let d = b.state_duration(a.state).unwrap_or(0.0) - 1.0;
        b.set_state_duration(a.state, Some(d));
    }
    let Some(lm) = b.p(t).last_move else { return Val::False };
    if let Some(slot) = b.p(t).move_slots.iter().find(|s| s.id == lm) {
        if slot.pp == 0 {
            return Val::False;
        }
    }
    let n = b.d.moves[lm as usize].name.clone();
    if let Some((an, s)) = ability_from(b, a) {
        add!(b, "-start", t, "Disable", n, format!("[from] ability: {an}"), format!("[of] {s}"));
    } else {
        add!(b, "-start", t, "Disable", n);
    }
    b.sset(a.state, K::Move, Val::MoveId(lm));
    Val::Undef
}
fn disable_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Disable");
    Val::Undef
}
fn disable_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.sget(a.state, K::Move) == Val::MoveId(b.mv(mv).def) && b.mv(mv).flags & mflag::CANTUSETWICE == 0 {
        let t = tgt(a);
        let n = b.move_def(mv).name.clone();
        add!(b, "cant", t, "Disable", n);
        return Val::False;
    }
    Val::Undef
}
fn disable_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Val::MoveId(mv) = b.sget(a.state, K::Move) {
        if b.has_move(t, mv) {
            b.disable_move(t, mv, false, None);
        }
    }
    Val::Undef
}
fn throatchop_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "Throat Chop", "[silent]");
    Val::Undef
}
fn throatchop_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let ids: Vec<u16> = b.p(t).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        if b.d.moves[id as usize].flags & mflag::SOUND != 0 {
            b.disable_move(t, id, false, None);
        }
    }
    Val::Undef
}
fn throatchop_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::SOUND != 0 {
        let t = tgt(a);
        add!(b, "cant", t, "move: Throat Chop");
        return Val::False;
    }
    Val::Undef
}
fn throatchop_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Throat Chop", "[silent]");
    Val::Undef
}
fn torment_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "Torment");
    Val::Undef
}
fn torment_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Torment");
    Val::Undef
}
fn torment_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some(lm) = b.p(t).last_move {
        if b.d.moves[lm as usize].id != "struggle" {
            b.disable_move(t, lm, false, None);
        }
    }
    Val::Undef
}
fn healblock_duration(b: &mut Battle, a: &HArgs) -> Val {
    if b.ename(a.effect) == "Psychic Noise" {
        return Val::Num(2.0);
    }
    if let Some(s) = src(a) {
        if b.has_ability(s, "persistent") {
            add!(b, "-activate", s, "ability: Persistent", "[move] Heal Block");
            return Val::Num(7.0);
        }
    }
    Val::Num(5.0)
}
fn healblock_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "move: Heal Block");
    if let Some(s) = src(a) {
        b.pm(s).move_this_turn_result = Val::True;
    }
    Val::Undef
}
fn healblock_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let ids: Vec<u16> = b.p(t).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        if b.d.moves[id as usize].flags & mflag::HEAL != 0 {
            b.disable_move(t, id, false, None);
        }
    }
    Val::Undef
}
fn healblock_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::HEAL != 0 {
        let t = tgt(a);
        let n = b.move_def(mv).name.clone();
        add!(b, "cant", t, "move: Heal Block", n);
        return Val::False;
    }
    Val::Undef
}
fn healblock_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "move: Heal Block");
    Val::Undef
}
fn healblock_try_heal(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.eid(a.effect) == "zpower" {
        return a.relay;
    }
    if let Some(s) = src(a) {
        if t != s && b.p(t).hp != b.p(t).maxhp && b.ename(a.effect) == "Pollen Puff" {
            b.attr_last_move(&["[still]"]);
            add!(b, "cant", s, "move: Heal Block", "Pollen Puff");
            return Val::Null;
        }
    }
    Val::False
}
fn healblock_restart(b: &mut Battle, a: &HArgs) -> Val {
    if b.ename(a.effect) == "Psychic Noise" {
        return Val::Undef;
    }
    let t = tgt(a);
    add!(b, "-fail", t, "move: Heal Block");
    if let Some(s) = src(a) {
        if !b.p(s).move_this_turn_result.truthy() {
            b.pm(s).move_this_turn_result = Val::False;
        }
    }
    Val::Undef
}
fn yawn_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-start", t, "move: Yawn", format!("[of] {s}"));
    Val::Undef
}
fn yawn_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "move: Yawn", "[silent]");
    let source = b.state_source(a.state);
    let slp = b.d.c.slp;
    b.try_set_status(t, slp, source, EffRef::NONE);
    Val::Undef
}
fn perishsong_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "perish0");
    b.faint(t, None, EffRef::NONE);
    Val::Undef
}
fn perishsong_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = b.state_duration(a.state).unwrap_or(0.0);
    add!(b, "-start", t, format!("perish{}", d as i32));
    Val::Undef
}
fn roost_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "move: Roost");
    Val::Undef
}
fn roost_type(b: &mut Battle, a: &HArgs) -> Val {
    let Val::Types(ts, n) = a.relay else { return Val::Undef };
    let flying = b.d.type_id("Flying");
    let mut out = [NO_TYPE; 3];
    let mut k = 0;
    for &t in &ts[..n as usize] {
        if t != flying {
            out[k] = t;
            k += 1;
        }
    }
    b.sset(a.state, K::TypeWas, Val::Types(ts, n));
    Val::Types(out, k as u8)
}
fn smackdown_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let mut applies = false;
    if b.has_type(m, "Flying") || b.has_ability_any(m, &["levitate", "eelevate"]) {
        applies = true;
    }
    if b.has_item(m, "ironball") || b.has_vol(m, "ingrain") || b.has_pseudo("gravity") {
        applies = false;
    }
    if b.remove_volatile_id(m, "fly") || b.remove_volatile_id(m, "bounce") {
        applies = true;
        b.cancel_move(m);
        let c = b.d.c.twoturnmove;
        b.remove_volatile(m, c);
    }
    if b.has_vol(m, "magnetrise") {
        applies = true;
        b.delete_volatile(m, "magnetrise");
    }
    if b.has_vol(m, "telekinesis") {
        applies = true;
        b.delete_volatile(m, "telekinesis");
    }
    if !applies {
        return Val::False;
    }
    add!(b, "-start", m, "Smack Down");
    Val::Undef
}
fn smackdown_restart(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.remove_volatile_id(m, "fly") || b.remove_volatile_id(m, "bounce") {
        b.cancel_move(m);
        let c = b.d.c.twoturnmove;
        b.remove_volatile(m, c);
        add!(b, "-start", m, "Smack Down");
    }
    Val::Undef
}
fn focusenergy_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "dragoncheer") {
        return Val::False;
    }
    let e = b.eid(a.effect);
    if e == "zpower" {
        add!(b, "-start", t, "move: Focus Energy", "[zeffect]");
    } else if matches!(e, "costar" | "imposter" | "psychup" | "transform") {
        add!(b, "-start", t, "move: Focus Energy", "[silent]");
    } else {
        add!(b, "-start", t, "move: Focus Energy");
    }
    Val::Undef
}
fn focusenergy_crit(_: &mut Battle, a: &HArgs) -> Val {
    Val::Num(a.relay.num() + 2.0)
}
fn dragoncheer_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "focusenergy") {
        return Val::False;
    }
    let e = b.eid(a.effect);
    if matches!(e, "costar" | "imposter" | "psychup" | "transform") {
        add!(b, "-start", t, "move: Dragon Cheer", "[silent]");
    } else {
        add!(b, "-start", t, "move: Dragon Cheer");
    }
    let dragon = b.has_type(t, "Dragon");
    b.sset(a.state, K::HasDragonType, Val::from_bool(dragon));
    Val::Undef
}
fn dragoncheer_crit(b: &mut Battle, a: &HArgs) -> Val {
    let add_n = if b.sget(a.state, K::HasDragonType).truthy() { 2.0 } else { 1.0 };
    Val::Num(a.relay.num() + add_n)
}
fn glaiverush_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singlemove", t, "Glaive Rush", "[silent]");
    Val::Undef
}
fn glaiverush_before_move(b: &mut Battle, a: &HArgs) -> Val {
    b.remove_volatile_id(tgt(a), "glaiverush");
    Val::Undef
}
fn minimize_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::bit("minimize") != 0 {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
fn minimize_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::bit("minimize") != 0 {
            return Val::True;
        }
    }
    a.relay
}
fn stockpile_boost(b: &mut Battle, a: &HArgs) {
    let t = tgt(a);
    let (cd, cs) = (b.p(t).boosts[1], b.p(t).boosts[3]);
    let mut bo = SparseBoosts::default();
    bo.set(B_DEF, 1);
    bo.set(B_SPD, 1);
    b.boost(bo, Some(t), Some(t), EffRef::NONE, false, false);
    if cd != b.p(t).boosts[1] {
        let v = b.sget(a.state, K::X1).num() - 1.0;
        b.sset(a.state, K::X1, Val::Num(v));
    }
    if cs != b.p(t).boosts[3] {
        let v = b.sget(a.state, K::X2).num() - 1.0;
        b.sset(a.state, K::X2, Val::Num(v));
    }
}
fn stockpile_start(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Layers, Val::Num(1.0));
    b.sset(a.state, K::X1, Val::Num(0.0));
    b.sset(a.state, K::X2, Val::Num(0.0));
    let t = tgt(a);
    add!(b, "-start", t, "stockpile1");
    stockpile_boost(b, a);
    Val::Undef
}
fn stockpile_restart(b: &mut Battle, a: &HArgs) -> Val {
    let l = b.sget(a.state, K::Layers).num();
    if l >= 3.0 {
        return Val::False;
    }
    b.sset(a.state, K::Layers, Val::Num(l + 1.0));
    let t = tgt(a);
    add!(b, "-start", t, format!("stockpile{}", (l + 1.0) as i32));
    stockpile_boost(b, a);
    Val::Undef
}
fn stockpile_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let (dd, sd) = (b.sget(a.state, K::X1).num(), b.sget(a.state, K::X2).num());
    if dd != 0.0 || sd != 0.0 {
        let mut bo = SparseBoosts::default();
        if dd != 0.0 {
            bo.set(B_DEF, dd as i8);
        }
        if sd != 0.0 {
            bo.set(B_SPD, sd as i8);
        }
        b.boost(bo, Some(t), Some(t), EffRef::NONE, false, false);
    }
    add!(b, "-end", t, "Stockpile");
    Val::Undef
}
fn imprison_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "move: Imprison");
    Val::Undef
}
fn imprison_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = b.state_source(a.state) else { return Val::Undef };
    let ids: Vec<u16> = b.p(s).move_slots.iter().map(|x| x.id).collect();
    for id in ids {
        if b.d.moves[id as usize].id == "struggle" {
            continue;
        }
        b.disable_move(t, id, true, None);
    }
    b.pm(t).maybe_disabled = true;
    Val::Undef
}
fn imprison_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    let Some(s) = b.state_source(a.state) else { return Val::Undef };
    let id = b.mv(mv).def;
    if b.move_def(mv).id != "struggle" && b.has_move(s, id) {
        let t = tgt(a);
        let n = b.move_def(mv).name.clone();
        add!(b, "cant", t, "move: Imprison", n);
        return Val::False;
    }
    Val::Undef
}
fn flashfire_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "ability: Flash Fire");
    Val::Undef
}
fn flashfire_mod(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some(mv) = amv(a) {
        if b.mv(mv).type_ == b.d.type_id("Fire") && b.has_ability(t, "flashfire") {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn flashfire_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "ability: Flash Fire", "[silent]");
    Val::Undef
}
fn unburden_spe(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).item == b.d.c.empty && !b.ignoring_ability(t) {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn charge_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let n = b.ename(a.effect);
    if matches!(n, "Electromorphosis" | "Wind Power") {
        let mvn = b.active_move.map(|m| b.move_def(m).name.clone()).unwrap_or_default();
        add!(b, "-start", t, "Charge", mvn, format!("[from] ability: {n}"));
    } else {
        add!(b, "-start", t, "Charge");
    }
    Val::Undef
}
fn charge_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).type_ == b.d.type_id("Electric") {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
fn charge_after(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).type_ == b.d.type_id("Electric") && b.move_def(mv).id != "charge" {
            b.remove_volatile_id(tgt(a), "charge");
        }
    }
    Val::Undef
}
fn charge_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Charge", "[silent]");
    Val::Undef
}
fn attract_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::False };
    let (gt, gs) = (b.p(t).gender, b.p(s).gender);
    if !(gt == "M" && gs == "F") && !(gt == "F" && gs == "M") {
        return Val::False;
    }
    if !b.run_event(Ev::Attract, Arg::Mon(t), Some(s), EffRef::NONE, Val::Undef).truthy() {
        return Val::False;
    }
    let ss = b.mon_str(s);
    match b.ename(a.effect) {
        "Cute Charm" => add!(b, "-start", t, "Attract", "[from] ability: Cute Charm", format!("[of] {ss}")),
        "Destiny Knot" => add!(b, "-start", t, "Attract", "[from] item: Destiny Knot", format!("[of] {ss}")),
        _ => add!(b, "-start", t, "Attract"),
    }
    Val::Undef
}
fn attract_update(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some(s) = b.state_source(a.state) {
        if !b.p(s).is_active && b.has_vol(t, "attract") {
            b.remove_volatile_id(t, "attract");
        }
    }
    Val::Undef
}
fn attract_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let s = b.state_source(a.state).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-activate", t, "move: Attract", format!("[of] {s}"));
    if b.random_chance(1, 2) {
        add!(b, "cant", t, "Attract");
        return Val::False;
    }
    Val::Undef
}
fn attract_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Attract", "[silent]");
    Val::Undef
}
fn healingwish_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    b.single_event(Ev::Swap, EffRef::eff(a.this_eff), a.state, Arg::Mon(t), Arg::None, EffRef::NONE, Val::Undef);
    Val::Undef
}
fn healingwish_swap(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let p = b.p(t);
    if !p.fainted && (p.hp < p.maxhp || p.status != b.d.c.empty) {
        let mh = b.p(t).maxhp as f64;
        b.mon_heal(t, mh);
        b.clear_status(t);
        add!(b, "-heal", t, Health(t), "[from] move: Healing Wish");
        let hw = b.d.cond("healingwish").unwrap();
        let pos = b.p(t).position;
        b.remove_slot_condition(t.s, pos, hw);
    }
    Val::Undef
}

// ------------------------------------------------------------------
// side conditions
// ------------------------------------------------------------------
fn screen_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        if b.has_item(s, "lightclay") {
            return Val::Num(8.0);
        }
    }
    Val::Num(5.0)
}
fn holder_side(a: &HArgs) -> u8 {
    match a.holder {
        Holder::Side(s) => s,
        Holder::Mon(m) => m.s,
        _ => 0,
    }
}
fn screen_applies(b: &mut Battle, a: &HArgs, cat: Option<Category>) -> Option<u16> {
    // onAnyModifyDamage(damage, source, target, move): source = event target (attacker), target = event source
    let attacker = tgt(a);
    let target = src(a)?;
    let mv = amv(a)?;
    if target == attacker || target.s != holder_side(a) {
        return None;
    }
    if let Some(c) = cat {
        if b.mv(mv).category != c {
            return None;
        }
    }
    Some(mv)
}
fn screen_weaken(b: &mut Battle, a: &HArgs, mv: u16) {
    let target = src(a).unwrap();
    if !b.hit_data(mv, target).crit && !b.mv(mv).infiltrates {
        b.chain_modify(2732.0, 4096.0);
    }
}
fn reflect_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = screen_applies(b, a, Some(Category::Physical)) {
        screen_weaken(b, a, mv);
    }
    Val::Undef
}
fn lightscreen_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = screen_applies(b, a, Some(Category::Special)) {
        screen_weaken(b, a, mv);
    }
    Val::Undef
}
fn auroraveil_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = screen_applies(b, a, None) {
        let target = src(a).unwrap();
        let cat = b.mv(mv).category;
        if (b.has_side_condition(target.s, "reflect") && cat == Category::Physical) || (b.has_side_condition(target.s, "lightscreen") && cat == Category::Special) {
            return Val::Undef;
        }
        screen_weaken(b, a, mv);
    }
    Val::Undef
}
pub fn side_str(b: &Battle, s: u8) -> String {
    format!("p{}: {}", s + 1, b.sides[s as usize].name)
}
fn reflect_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "Reflect");
    Val::Undef
}
fn reflect_side_end(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sideend", s, "Reflect");
    Val::Undef
}
fn lightscreen_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Light Screen");
    Val::Undef
}
fn lightscreen_side_end(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sideend", s, "move: Light Screen");
    Val::Undef
}
fn auroraveil_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Aurora Veil");
    Val::Undef
}
fn auroraveil_side_end(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sideend", s, "move: Aurora Veil");
    Val::Undef
}
fn tailwind_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        if b.has_ability(s, "persistent") {
            add!(b, "-activate", s, "ability: Persistent", "[move] Tailwind");
            return Val::Num(6.0);
        }
    }
    Val::Num(4.0)
}
fn tailwind_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    if src(a).map(|x| b.has_ability(x, "persistent")).unwrap_or(false) {
        add!(b, "-sidestart", s, "move: Tailwind", "[persistent]");
    } else {
        add!(b, "-sidestart", s, "move: Tailwind");
    }
    Val::Undef
}
fn tailwind_side_end(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sideend", s, "move: Tailwind");
    Val::Undef
}

fn toxicspikes_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Toxic Spikes");
    b.sset(a.state, K::Layers, Val::Num(1.0));
    Val::Undef
}
fn toxicspikes_side_restart(b: &mut Battle, a: &HArgs) -> Val {
    let l = b.sget(a.state, K::Layers).num();
    if l >= 2.0 {
        return Val::False;
    }
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Toxic Spikes");
    b.sset(a.state, K::Layers, Val::Num(l + 1.0));
    Val::Undef
}
fn toxicspikes_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_grounded(m, false) != Some(true) {
        return Val::Undef;
    }
    if b.has_type(m, "Poison") {
        let s = side_str(b, m.s);
        let ms = b.mon_str(m);
        add!(b, "-sideend", s, "move: Toxic Spikes", format!("[of] {ms}"));
        let ts = b.cond("toxicspikes");
        b.remove_side_condition(m.s, ts);
    } else if b.has_type(m, "Steel") || b.has_item(m, "heavydutyboots") {
    } else {
        let foe0 = b.active_at(1 - m.s as usize, 0);
        let st = if b.sget(a.state, K::Layers).num() >= 2.0 { b.d.c.tox } else { b.d.c.psn };
        b.try_set_status(m, st, foe0, EffRef::NONE);
    }
    Val::Undef
}

// ------------------------------------------------------------------
// field
// ------------------------------------------------------------------
fn trickroom_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = tgt_or_src(a) {
        if b.has_ability(s, "persistent") {
            add!(b, "-activate", s, "ability: Persistent", "[move] Trick Room");
            return Val::Num(7.0);
        }
    }
    Val::Num(5.0)
}
/// durationCallback(source, effect) for field effects: the first argument is the source
fn tgt_or_src(a: &HArgs) -> Option<Mon> {
    a.target.mon().or(a.source.mon())
}
fn trickroom_field_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    if src(a).map(|x| b.has_ability(x, "persistent")).unwrap_or(false) {
        add!(b, "-fieldstart", "move: Trick Room", format!("[of] {s}"), "[persistent]");
    } else {
        add!(b, "-fieldstart", "move: Trick Room", format!("[of] {s}"));
    }
    Val::Undef
}
fn trickroom_field_restart(b: &mut Battle, _: &HArgs) -> Val {
    let tr_ = b.d.c.trickroom;
    b.remove_pseudo_weather(tr_);
    Val::Undef
}
fn trickroom_field_end(b: &mut Battle, _: &HArgs) -> Val {
    add!(b, "-fieldend", "move: Trick Room");
    Val::Undef
}
fn gravity_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = tgt_or_src(a) {
        if b.has_ability(s, "persistent") {
            add!(b, "-activate", s, "ability: Persistent", "[move] Gravity");
            return Val::Num(7.0);
        }
    }
    Val::Num(5.0)
}
fn gravity_field_start(b: &mut Battle, a: &HArgs) -> Val {
    if src(a).map(|x| b.has_ability(x, "persistent")).unwrap_or(false) {
        add!(b, "-fieldstart", "move: Gravity", "[persistent]");
    } else {
        add!(b, "-fieldstart", "move: Gravity");
    }
    for m in b.all_active() {
        let mut applies = false;
        if b.remove_volatile_id(m, "bounce") || b.remove_volatile_id(m, "fly") {
            applies = true;
            b.cancel_move(m);
            let c = b.d.c.twoturnmove;
            b.remove_volatile(m, c);
        }
        if b.has_vol(m, "magnetrise") {
            applies = true;
            b.delete_volatile(m, "magnetrise");
        }
        if b.has_vol(m, "telekinesis") {
            applies = true;
            b.delete_volatile(m, "telekinesis");
        }
        if applies {
            add!(b, "-activate", m, "move: Gravity");
        }
    }
    Val::Undef
}
fn gravity_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    b.chain_modify(6840.0, 4096.0);
    Val::Undef
}
fn gravity_disable(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let ids: Vec<u16> = b.p(t).move_slots.iter().map(|s| s.id).collect();
    for id in ids {
        if b.d.moves[id as usize].flags & mflag::bit("gravity") != 0 {
            b.disable_move(t, id, false, None);
        }
    }
    Val::Undef
}
fn gravity_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::bit("gravity") != 0 {
        let t = tgt(a);
        let n = b.move_def(mv).name.clone();
        add!(b, "cant", t, "move: Gravity", n);
        return Val::False;
    }
    Val::Undef
}
fn gravity_field_end(b: &mut Battle, _: &HArgs) -> Val {
    add!(b, "-fieldend", "move: Gravity");
    Val::Undef
}
fn item_duration(b: &mut Battle, a: &HArgs, item: &str, long: f64, short: f64) -> Val {
    if let Some(s) = tgt_or_src(a) {
        if b.has_item(s, item) {
            return Val::Num(long);
        }
    }
    Val::Num(short)
}
fn rain_duration(b: &mut Battle, a: &HArgs) -> Val {
    item_duration(b, a, "damprock", 8.0, 5.0)
}
fn sun_duration(b: &mut Battle, a: &HArgs) -> Val {
    item_duration(b, a, "heatrock", 8.0, 5.0)
}
fn sand_duration(b: &mut Battle, a: &HArgs) -> Val {
    item_duration(b, a, "smoothrock", 8.0, 5.0)
}
fn snow_duration(b: &mut Battle, a: &HArgs) -> Val {
    item_duration(b, a, "icyrock", 8.0, 5.0)
}
fn terrain_duration(b: &mut Battle, a: &HArgs) -> Val {
    item_duration(b, a, "terrainextender", 8.0, 5.0)
}
/// onWeatherModifyDamage(damage, attacker, defender, move): attacker = event target
fn rain_damage(b: &mut Battle, a: &HArgs) -> Val {
    let Some(def) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.effective_weather(def, EffRef::NONE, false) != "raindance" {
        return Val::Undef;
    }
    let t = b.mv(mv).type_;
    if t == b.d.type_id("Water") {
        b.chain_modify(1.5, 1.0);
    } else if t == b.d.type_id("Fire") {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn sun_damage(b: &mut Battle, a: &HArgs) -> Val {
    let att = tgt(a);
    let Some(def) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.move_def(mv).id == "hydrosteam" && b.effective_weather(att, EffRef::NONE, false) == "sunnyday" {
        b.chain_modify(1.5, 1.0);
        return Val::Undef;
    }
    if b.effective_weather(def, EffRef::NONE, false) != "sunnyday" {
        return Val::Undef;
    }
    let t = b.mv(mv).type_;
    if t == b.d.type_id("Fire") {
        b.chain_modify(1.5, 1.0);
    } else if t == b.d.type_id("Water") {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn sun_immunity(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.effective_weather(t, EffRef::NONE, false) != "sunnyday" {
        return Val::Undef;
    }
    if a.relay == Val::Key("frz") {
        return Val::False;
    }
    Val::Undef
}
fn sand_spd(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_type(t, "Rock") && b.effective_weather(t, EffRef::NONE, false) == "sandstorm" {
        return Val::Num(b.modify(a.relay.num(), 1.5));
    }
    Val::Undef
}
fn snow_def(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_type(t, "Ice") && b.effective_weather(t, EffRef::NONE, false) == "snowscape" {
        return Val::Num(b.modify(a.relay.num(), 1.5));
    }
    Val::Undef
}
fn weather_field_start(b: &mut Battle, a: &HArgs) -> Val {
    let name = b.eff(a.this_eff).name.clone();
    if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-weather", name, format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-weather", name);
    }
    Val::Undef
}
fn weather_field_residual(b: &mut Battle, a: &HArgs) -> Val {
    let name = b.eff(a.this_eff).name.clone();
    let id = b.eff(a.this_eff).id.clone();
    add!(b, "-weather", name, "[upkeep]");
    let check = matches!(id.as_str(), "sandstorm" | "hail" | "snowscape");
    if !check || b.is_weather(&[id.as_str()]) {
        b.each_event(Ev::Weather, EffRef::NONE);
    }
    Val::Undef
}
fn weather_chip(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let d = b.p(t).base_maxhp as f64 / 16.0;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn weather_field_end(b: &mut Battle, _: &HArgs) -> Val {
    add!(b, "-weather", "none");
    Val::Undef
}
fn terrain_field_start(b: &mut Battle, a: &HArgs) -> Val {
    let name = b.eff(a.this_eff).name.clone();
    if let Some((n, s)) = ability_from(b, a) {
        add!(b, "-fieldstart", format!("move: {name}"), format!("[from] ability: {n}"), format!("[of] {s}"));
    } else {
        add!(b, "-fieldstart", format!("move: {name}"));
    }
    Val::Undef
}
fn terrain_field_end(b: &mut Battle, a: &HArgs) -> Val {
    let name = b.eff(a.this_eff).name.clone();
    if b.eff(a.this_eff).id == "mistyterrain" {
        add!(b, "-fieldend", "Misty Terrain");
    } else {
        add!(b, "-fieldend", format!("move: {name}"));
    }
    Val::Undef
}
/// onTryHit(target, source, effect)
fn psychicterrain_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).priority <= 0.1 || b.mv(mv).target == MoveTarget::SelfTarget {
        return Val::Undef;
    }
    if b.is_semi_invulnerable(target) || target.s == source.s {
        return Val::Undef;
    }
    if b.is_grounded(target, false) != Some(true) {
        if b.move_def(mv).priority > 0.0 {
            b.hint("Psychic Terrain doesn't affect airborne Pokémon.", false);
        }
        return Val::Undef;
    }
    add!(b, "-activate", target, "move: Psychic Terrain");
    Val::Null
}
fn psychicterrain_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let att = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).type_ == b.d.type_id("Psychic") && b.is_grounded(att, false) == Some(true) && !b.is_semi_invulnerable(att) {
        b.chain_modify(5325.0, 4096.0);
    }
    Val::Undef
}
fn grassyterrain_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let att = tgt(a);
    let Some(def) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    let id = b.move_def(mv).id.as_str();
    if matches!(id, "earthquake" | "bulldoze" | "magnitude") && b.is_grounded(def, false) == Some(true) && !b.is_semi_invulnerable(def) {
        b.chain_modify(0.5, 1.0);
        return Val::Undef;
    }
    if b.mv(mv).type_ == b.d.type_id("Grass") && b.is_grounded(att, false) == Some(true) {
        b.chain_modify(5325.0, 4096.0);
    }
    Val::Undef
}
fn grassyterrain_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_grounded(m, false) == Some(true) && !b.is_semi_invulnerable(m) {
        let h = b.p(m).base_maxhp as f64 / 16.0;
        b.heal(h, Some(m), Some(m), EffRef::NONE);
    }
    Val::Undef
}
/// onSetStatus(status, target, source, effect)
fn electricterrain_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if a.relay == Val::Eff(b.d.c.slp) && b.is_grounded(t, false) == Some(true) && !b.is_semi_invulnerable(t) {
        let et = b.etype(a.effect);
        let no_secs = a.effect.is_move() && b.mv(a.effect.mv).secondaries.is_none();
        if b.eid(a.effect) == "yawn" || (et == EffType::Move && no_secs) {
            add!(b, "-activate", t, "move: Electric Terrain");
        }
        return Val::False;
    }
    Val::Undef
}
fn electricterrain_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.is_grounded(t, false) != Some(true) || b.is_semi_invulnerable(t) {
        return Val::Undef;
    }
    if let Val::Eff(e) = a.relay {
        if b.eff(e).id == "yawn" {
            add!(b, "-activate", t, "move: Electric Terrain");
            return Val::Null;
        }
    }
    Val::Undef
}
fn electricterrain_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let att = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).type_ == b.d.type_id("Electric") && b.is_grounded(att, false) == Some(true) && !b.is_semi_invulnerable(att) {
        b.chain_modify(5325.0, 4096.0);
    }
    Val::Undef
}
fn mistyterrain_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.is_grounded(t, false) != Some(true) || b.is_semi_invulnerable(t) {
        return Val::Undef;
    }
    let has_status = b.move_status_of(a.effect).is_some();
    if !a.effect.is_none() && (has_status || b.eid(a.effect) == "yawn") {
        add!(b, "-activate", t, "move: Misty Terrain");
    }
    Val::False
}
fn mistyterrain_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.is_grounded(t, false) != Some(true) || b.is_semi_invulnerable(t) {
        return Val::Undef;
    }
    if a.relay == Val::Eff(b.d.c.confusion) {
        let et = b.etype(a.effect);
        let no_secs = a.effect.is_move() && b.mv(a.effect.mv).secondaries.is_none();
        if et == EffType::Move && no_secs {
            add!(b, "-activate", t, "move: Misty Terrain");
        }
        return Val::Null;
    }
    Val::Undef
}
fn mistyterrain_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let Some(def) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).type_ == b.d.type_id("Dragon") && b.is_grounded(def, false) == Some(true) && !b.is_semi_invulnerable(def) {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}

// ------------------------------------------------------------------ added for the M-A / M-B team pool
fn leechseed_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "move: Leech Seed");
    Val::Undef
}
fn leechseed_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let slot = b.state(a.state).and_then(|s| s.source_slot);
    let Some((ss, pos)) = slot else { return Val::Undef };
    let Some(target) = b.active_at(ss as usize, pos as usize) else { return Val::Undef };
    if b.p(target).fainted || b.p(target).hp <= 0 {
        return Val::Undef;
    }
    let d = b.p(m).base_maxhp as f64 / 8.0;
    let dealt = b.damage(d, Some(m), Some(target), EffRef::NONE, false);
    let n = dealt.num();
    if n > 0.0 {
        b.heal(n, Some(target), Some(m), EffRef::NONE);
    }
    Val::Undef
}
fn destinybond_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singlemove", t, "Destiny Bond");
    Val::Undef
}
fn destinybond_faint(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if a.effect.is_none() || b.is_ally(t, Some(s)) {
        return Val::Undef;
    }
    if b.etype(a.effect) == EffType::Move {
        add!(b, "-activate", t, "move: Destiny Bond");
        b.faint(s, None, EffRef::NONE);
    }
    Val::Undef
}
fn destinybond_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if let Some(mv) = if a.effect.is_move() { Some(a.effect.mv) } else { None } {
        if b.move_def(mv).id == "destinybond" {
            return Val::Undef;
        }
    }
    b.remove_volatile_id(m, "destinybond");
    Val::Undef
}
fn destinybond_aborted(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.remove_volatile_id(m, "destinybond");
    Val::Undef
}
fn stealthrock_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Stealth Rock");
    Val::Undef
}
fn stealthrock_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_item(m, "heavydutyboots") {
        return Val::Undef;
    }
    let Some(id) = b.d.move_id("stealthrock") else { return Val::Undef };
    let am = b.new_active_move(id);
    let tm = b.run_effectiveness(m, am).clamp(-6, 6);
    let d = b.p(m).maxhp as f64 * 2f64.powi(tm) / 8.0;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}
fn spikes_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "Spikes");
    b.sset(a.state, K::Layers, Val::Num(1.0));
    Val::Undef
}
fn spikes_side_restart(b: &mut Battle, a: &HArgs) -> Val {
    let l = b.sget(a.state, K::Layers).num();
    if l >= 3.0 {
        return Val::False;
    }
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "Spikes");
    b.sset(a.state, K::Layers, Val::Num(l + 1.0));
    Val::Undef
}
fn spikes_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_grounded(m, false) != Some(true) || b.has_item(m, "heavydutyboots") {
        return Val::Undef;
    }
    let l = b.sget(a.state, K::Layers).num() as usize;
    let amt = [0.0, 3.0, 4.0, 6.0][l.min(3)];
    let d = amt * b.p(m).maxhp as f64 / 24.0;
    b.damage(d, None, None, EffRef::NONE, false);
    Val::Undef
}

// ------------------------------------------------------------------ batch 4 (rare moves)
fn curse_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    match src(a) {
        Some(s) => {
            let ss = b.mon_str(s);
            add!(b, "-start", t, "Curse", format!("[of] {ss}"));
        }
        None => add!(b, "-start", t, "Curse"),
    }
    Val::Undef
}
fn curse_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let d = b.p(m).base_maxhp as f64 / 4.0;
    b.damage(d, Some(m), None, EffRef::NONE, false);
    Val::Undef
}
fn safeguard_duration(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(s) = src(a) {
        if b.has_ability(s, "persistent") {
            add!(b, "-activate", s, "ability: Persistent", "[move] Safeguard");
            return Val::Num(7.0);
        }
    }
    Val::Num(5.0)
}
fn safeguard_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if a.effect.is_none() {
        return Val::Undef;
    }
    if b.eid(a.effect) == "yawn" {
        return Val::Undef;
    }
    let is_move = b.etype(a.effect) == EffType::Move;
    if is_move && a.effect.is_move() && b.mv(a.effect.mv).infiltrates && !b.is_ally(t, Some(s)) {
        return Val::Undef;
    }
    if t != s {
        let secondaries = a.effect.is_move() && b.mv(a.effect.mv).secondaries.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        if b.eid(a.effect) == "synchronize" || (is_move && !secondaries) {
            add!(b, "-activate", t, "move: Safeguard");
        }
        return Val::Null;
    }
    Val::Undef
}
fn safeguard_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if a.effect.is_none() {
        return Val::Undef;
    }
    let is_move = b.etype(a.effect) == EffType::Move;
    if is_move && a.effect.is_move() && b.mv(a.effect.mv).infiltrates && !b.is_ally(t, Some(s)) {
        return Val::Undef;
    }
    let Val::Eff(st) = a.relay else { return Val::Undef };
    let sid = b.eff(st).id.clone();
    if (sid == "confusion" || sid == "yawn") && t != s {
        let secondaries = a.effect.is_move() && b.mv(a.effect.mv).secondaries.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        if is_move && !secondaries {
            add!(b, "-activate", t, "move: Safeguard");
        }
        return Val::Null;
    }
    Val::Undef
}
fn safeguard_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    let persistent = src(a).map(|m| b.has_ability(m, "persistent")).unwrap_or(false);
    if persistent {
        add!(b, "-sidestart", s, "Safeguard", "[persistent]");
    } else {
        add!(b, "-sidestart", s, "Safeguard");
    }
    Val::Undef
}
fn safeguard_side_end(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sideend", s, "Safeguard");
    Val::Undef
}
fn stickyweb_side_start(b: &mut Battle, a: &HArgs) -> Val {
    let s = side_str(b, holder_side(a));
    add!(b, "-sidestart", s, "move: Sticky Web");
    Val::Undef
}
fn stickyweb_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_grounded(m, false) != Some(true) || b.has_item(m, "heavydutyboots") {
        return Val::Undef;
    }
    add!(b, "-activate", m, "move: Sticky Web");
    let foe = b.active_at(1 - m.s as usize, 0);
    let mid = b.d.move_id("stickyweb").unwrap_or(NONE16);
    let am = b.new_active_move(mid);
    let mref = EffRef { eff: b.mv(am).eff, mv: am, part: 0 };
    let bo = Battle::boosts1(B_SPE, -1);
    b.boost(bo, Some(m), foe, mref, false, false);
    Val::Undef
}
fn syrupbomb_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-start", m, "Syrup Bomb");
    Val::Undef
}
fn syrupbomb_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if let Some(s) = b.state_source(a.state) {
        if !b.p(s).is_active {
            b.remove_volatile_id(m, "syrupbomb");
        }
    }
    Val::Undef
}
fn syrupbomb_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let s = b.state_source(a.state);
    let bo = Battle::boosts1(B_SPE, -1);
    b.boost(bo, Some(m), s, EffRef::NONE, false, false);
    Val::Undef
}
fn syrupbomb_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-end", m, "Syrup Bomb", "[silent]");
    Val::Undef
}
fn wish_start(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    let hp = b.p(s).maxhp as f64 / 2.0;
    b.sset(a.state, K::Hp, Val::Num(hp));
    let turn = b.turn as f64;
    b.sset(a.state, K::StartTime, Val::Num(turn));
    Val::Undef
}
fn wish_residual(b: &mut Battle, a: &HArgs) -> Val {
    let start = b.sget(a.state, K::StartTime).num();
    if (b.turn as f64) <= start {
        return Val::Undef;
    }
    let slot = b.state(a.state).and_then(|s| s.source_slot);
    if let (Some((ss, pos)), Some(c)) = (slot, b.d.cond("wish")) {
        b.remove_slot_condition(ss, pos, c);
    }
    Val::Undef
}
fn wish_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if !b.p(t).fainted {
        let hp = b.sget(a.state, K::Hp).num();
        let healed = b.heal(hp, Some(t), Some(t), EffRef::NONE);
        if healed.truthy() {
            let wisher = b.state(a.state).and_then(|x| x.source).map(|s| b.p(s).name.clone()).unwrap_or_default();
            add!(b, "-heal", t, Health(t), "[from] move: Wish", format!("[wisher] {wisher}"));
        }
    }
    Val::Undef
}
fn bounce_invulnerability(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "gust" | "twister" | "skyuppercut" | "thunder" | "hurricane" | "smackdown" | "thousandarrows") {
            return Val::Undef;
        }
    }
    Val::False
}
fn bounce_source_bp(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "gust" | "twister") {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
fn noretreat_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-start", m, "move: No Retreat");
    Val::Undef
}
fn noretreat_trap(b: &mut Battle, a: &HArgs) -> Val {
    b.try_trap(tgt(a), false);
    Val::Undef
}

fn chillyreception_before_move(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.move_def(mv).id != "chillyreception" {
        return Val::Undef;
    }
    add!(b, "-prepare", u, "Chilly Reception", "[premajor]");
    Val::Undef
}

fn reflect_cond_start(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::TargetSlot, Val::Undef);
    b.sset(a.state, K::Hp, Val::Num(0.0));
    Val::Undef
}
fn reflect_redirect(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.move_def(mv).id != id {
        return Val::Undef;
    }
    let Some(s) = src(a) else { return Val::Undef };
    if Some(s) != b.state(a.state).and_then(|x| x.target_mon) {
        return Val::Undef;
    }
    let Val::Num(x) = b.sget(a.state, K::TargetSlot) else { return Val::Undef };
    let x = x as usize - 1;
    match b.active_at(x / 8, x % 8) {
        Some(m) => Val::Mon(m),
        None => Val::Undef,
    }
}
fn reflect_hit(b: &mut Battle, a: &HArgs, cat: Category) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if !b.is_ally(s, Some(t)) && b.move_def(mv).category == cat {
        let slot = s.s as f64 * 8.0 + b.p(s).position as f64 + 1.0;
        b.sset(a.state, K::TargetSlot, Val::Num(slot));
        b.sset(a.state, K::Hp, Val::Num(2.0 * a.relay.num()));
    }
    Val::Undef
}
fn mirrorcoat_redirect(b: &mut Battle, a: &HArgs) -> Val {
    reflect_redirect(b, a, "mirrorcoat")
}
fn mirrorcoat_hit(b: &mut Battle, a: &HArgs) -> Val {
    reflect_hit(b, a, Category::Special)
}
fn counter_redirect(b: &mut Battle, a: &HArgs) -> Val {
    reflect_redirect(b, a, "counter")
}
fn counter_hit(b: &mut Battle, a: &HArgs) -> Val {
    reflect_hit(b, a, Category::Physical)
}

fn focuspunch_start(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    add!(b, "-singleturn", u, "move: Focus Punch");
    Val::Undef
}
fn focuspunch_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category != Category::Status {
        b.sset(a.state, K::X1, Val::True);
    }
    Val::Undef
}
fn focuspunch_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if let Val::Eff(st) = a.relay {
        if b.eff(st).id == "flinch" {
            return Val::Null;
        }
    }
    Val::Undef
}

fn fling_update(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let item = b.p(u).item;
    let empty = b.d.c.empty;
    b.set_item(u, empty, None, EffRef::NONE);
    b.pm(u).last_item = item;
    b.pm(u).used_item_this_turn = true;
    let n = b.eff(item).name.clone();
    add!(b, "-enditem", u, n, "[from] move: Fling");
    b.run_event(Ev::AfterUseItem, Arg::Mon(u), None, EffRef::NONE, Val::Eff(item));
    let c = b.cond("fling");
    b.remove_volatile(u, c);
    Val::Undef
}

fn futuremove_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let slot = t.s as f64 * 8.0 + b.p(t).position as f64 + 1.0;
    b.sset(a.state, K::TargetSlot, Val::Num(slot));
    let end = b.turn as f64 - 1.0 + 2.0;
    b.sset(a.state, K::EndingTurn, Val::Num(end));
    Val::Undef
}
fn futuremove_residual(b: &mut Battle, a: &HArgs) -> Val {
    // getOverflowedTurnCount() = turn - 1 (mod 256)
    if ((b.turn as f64 - 1.0) % 256.0) < b.sget(a.state, K::EndingTurn).num() {
        return Val::Undef;
    }
    let Val::Num(x) = b.sget(a.state, K::TargetSlot) else { return Val::Undef };
    let x = x as usize - 1;
    let c = b.cond("futuremove");
    b.remove_slot_condition((x / 8) as u8, (x % 8) as u8, c);
    Val::Undef
}
fn futuremove_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(source) = b.state(a.state).and_then(|x| x.source) else { return Val::Undef };
    let Val::MoveId(def) = b.sget(a.state, K::Move) else { return Val::Undef };
    if b.p(t).fainted || t == source {
        return Val::Undef;
    }
    let name = b.d.moves[def as usize].name.clone();
    add!(b, "-end", t, format!("move: {name}"));
    b.remove_volatile_id(t, "protect");
    b.remove_volatile_id(t, "endure");
    let hit = b.new_active_move(def);
    b.mvm(hit).future_hit = true;
    // Showdown hits with the slot condition's moveData, which has ignoreImmunity: false (Future Sight's own move data
    // says true, for the turn it is used): Dark types are immune to the delayed hit
    b.mvm(hit).ignore_immunity = Some(IgnoreImmunity::No);
    if b.has_ability(source, "infiltrator") {
        b.mvm(hit).infiltrates = true;
    }
    if b.has_ability(source, "normalize") {
        let n = b.d.type_id("Normal");
        b.mvm(hit).type_ = n;
    }
    b.try_spread_move_hit(vec![t], source, hit, true);
    if b.p(source).is_active && b.has_item(source, "lifeorb") {
        let lo = b.p(source).item;
        b.single_event(Ev::AfterMoveSecondarySelf, EffRef::eff(lo), StateLoc::Item(source), Arg::Mon(source), Arg::Mon(t),
                       EffRef::eff(lo), Val::Undef);
    }
    b.active_move = None;
    b.check_win(None);
    Val::Undef
}

fn octolock_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let s = src(a).map(|s| b.mon_str(s)).unwrap_or_default();
    add!(b, "-start", t, "move: Octolock", format!("[of] {s}"));
    Val::Undef
}
fn octolock_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let source = b.state_source(a.state);
    if let Some(s) = source {
        if !b.p(s).is_active || b.p(s).hp == 0 || b.p(s).active_turns == 0 {
            let c = b.cond("octolock");
            b.pm(t).volatiles.retain(|x| x.0 != c);
            add!(b, "-end", t, "Octolock", "[partiallytrapped]", "[silent]");
            return Val::Undef;
        }
    }
    let mut bo = SparseBoosts::default();
    bo.set(B_DEF, -1);
    bo.set(B_SPD, -1);
    let oid = b.d.move_id("octolock").unwrap();
    let nm = b.new_active_move(oid);
    let mr = EffRef { eff: b.mv(nm).eff, mv: nm, part: 0 };
    b.boost(bo, Some(t), source, mr, false, false);
    Val::Undef
}
fn octolock_trap(b: &mut Battle, a: &HArgs) -> Val {
    if b.state_source(a.state).map(|s| b.p(s).is_active).unwrap_or(false) {
        b.try_trap(tgt(a), false);
    }
    Val::Undef
}
fn uproar_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "Uproar");
    Val::Undef
}
fn uproar_residual(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "throatchop") {
        b.remove_volatile_id(t, "uproar");
        return Val::Undef;
    }
    let struggle = b.d.move_id("struggle");
    if b.p(t).last_move.is_some() && b.p(t).last_move == struggle {
        let c = b.cond("uproar");
        b.pm(t).volatiles.retain(|x| x.0 != c);           // `delete target.volatiles['uproar']`: no End event
    }
    add!(b, "-start", t, "Uproar", "[upkeep]");
    Val::Undef
}
fn uproar_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Uproar");
    Val::Undef
}
fn uproar_any_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay != Val::Eff(b.d.c.slp) {
        return Val::Undef;
    }
    let m = tgt(a);
    if b.state(a.state).and_then(|x| x.target_mon) == Some(m) {
        add!(b, "-fail", m, "slp", "[from] Uproar", "[msg]");
    } else {
        add!(b, "-fail", m, "slp", "[from] Uproar");
    }
    Val::Null
}

fn magnetrise_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-start", t, "Magnet Rise");
    Val::Undef
}
fn magnetrise_immunity(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Type(b.d.type_id("Ground")) || a.relay == Val::Key("Ground") {
        return Val::False;
    }
    Val::Undef
}
fn magnetrise_end(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-end", t, "Magnet Rise");
    Val::Undef
}
fn swap_atk_def(b: &mut Battle, m: Mon) {
    let st = &mut b.pm(m).stored_stats;
    st.swap(1, 2);
}
fn powertrick_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-start", m, "Power Trick");
    swap_atk_def(b, m);
    Val::Undef
}
fn powertrick_swap(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    swap_atk_def(b, m);
    Val::Undef
}
fn powertrick_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-end", m, "Power Trick");
    swap_atk_def(b, m);
    Val::Undef
}
fn powertrick_restart(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.remove_volatile_id(m, "powertrick");
    Val::Undef
}
fn electrify_start(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-singleturn", t, "move: Electrify");
    Val::Undef
}
fn electrify_modify_type(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.move_def(mv).id != "struggle" {
        let e = b.d.type_id("Electric");
        b.mvm(mv).type_ = e;
    }
    Val::Undef
}
fn aquaring_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-start", m, "Aqua Ring");
    Val::Undef
}
fn aquaring_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 16.0;
    b.heal(h, Some(m), None, EffRef::NONE);
    Val::Undef
}

fn semi_weather_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("sandstorm") || a.relay == Val::Key("hail") {
        return Val::False;
    }
    Val::Undef
}
fn dig_invulnerability(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "earthquake" | "magnitude") {
            return Val::Undef;
        }
    }
    Val::False
}
fn dig_source_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "earthquake" | "magnitude") {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
fn beakblast_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-singleturn", m, "move: Beak Blast");
    Val::Undef
}
fn beakblast_hit(b: &mut Battle, a: &HArgs) -> Val {
    // onHit(target, source, move): contact burns the attacker
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) {
        let brn = b.d.c.brn;
        b.try_set_status(s, brn, Some(t), EffRef::NONE);
    }
    Val::Undef
}

fn saltcure_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-start", m, "Salt Cure");
    Val::Undef
}
fn saltcure_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let div = if b.has_type(m, "Water") || b.has_type(m, "Steel") { 8.0 } else { 16.0 };
    let d = b.p(m).base_maxhp as f64 / div;
    b.damage(d, Some(m), None, EffRef::NONE, false);
    Val::Undef
}
fn saltcure_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-end", m, "Salt Cure");
    Val::Undef
}
fn dive_invulnerability(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "surf" | "whirlpool") {
            return Val::Undef;
        }
    }
    Val::False
}
fn dive_source_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if matches!(b.move_def(mv).id.as_str(), "surf" | "whirlpool") {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
