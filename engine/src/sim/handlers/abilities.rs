//! Ability handlers (data/abilities.ts with the Champions mod), for the abilities in the team
//! pool and the abilities their Mega Evolutions get.
//!
//! Argument mapping: Showdown handlers get (relayVar, eventTarget, eventSource, effect). Here
//! `a.relay`, `a.target`, `a.source`, `a.effect`; the parameter names in the comments are
//! Showdown's.

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
        ("berserk", "onDamage") => berserk_damage,
        ("berserk", "onTryEatItem") => berserk_try_eat,
        ("berserk", "onAfterMoveSecondary") => berserk_after,
        ("magician", "onAfterMoveSecondarySelf") => magician_after,
        ("imposter", "onSwitchIn") => imposter_switch_in,
        ("mummy", "onDamagingHit") => mummy_hit,
        ("hungerswitch", "onResidual") => hungerswitch_residual,
        ("keeneye", "onTryBoost") | ("illuminate", "onTryBoost") => keeneye_try_boost,
        ("keeneye", "onModifyMove") | ("illuminate", "onModifyMove") => keeneye_modify_move,
        ("bigpecks", "onTryBoost") => bigpecks_try_boost,
        ("supersweetsyrup", "onStart") => supersweetsyrup_start,
        ("owntempo", "onHit") => noop_ab,
        ("zerotohero", "onSwitchIn") => zerotohero_switch_in,

        ("skilllink", "onModifyMove") => skilllink_modify_move,
        ("unseenfist", "onHitProtect") | ("piercingdrill", "onHitProtect") => unseenfist_hit_protect,
        ("innardsout", "onDamagingHit") => innardsout_hit,
        ("guts", "onModifyAtk") => guts_atk,
        ("marvelscale", "onModifyDef") => guts_atk,
        ("gooey", "onDamagingHit") => gooey_hit,
        ("tanglinghair", "onDamagingHit") => gooey_hit,
        ("waterbubble", "onSourceModifyAtk") | ("waterbubble", "onSourceModifySpA") => waterbubble_source,
        ("waterbubble", "onModifyAtk") | ("waterbubble", "onModifySpA") => waterbubble_mod,
        ("waterbubble", "onUpdate") => waterbubble_update,
        ("waterbubble", "onSetStatus") => waterbubble_set_status,
        ("dryskin", "onTryHit") => dryskin_try_hit,
        ("dryskin", "onSourceBasePower") => dryskin_bp,
        ("dryskin", "onWeather") => dryskin_weather,
        ("overcoat", "onImmunity") => overcoat_immunity,
        ("overcoat", "onTryHit") => overcoat_try_hit,
        ("eartheater", "onTryHit") => eartheater_try_hit,
        ("leafguard", "onSetStatus") => leafguard_set_status,
        ("leafguard", "onTryAddVolatile") => leafguard_try_add_volatile,
        ("insomnia", "onUpdate") => insomnia_update,
        ("insomnia", "onSetStatus") => insomnia_set_status,
        ("insomnia", "onTryAddVolatile") => insomnia_try_add_volatile,
        ("vitalspirit", "onUpdate") => vitalspirit_update,
        ("vitalspirit", "onSetStatus") => vitalspirit_set_status,
        ("vitalspirit", "onTryAddVolatile") => vitalspirit_try_add_volatile,
        ("zerotohero", "onSwitchOut") => zerotohero_switch_out,
        ("reckless", "onBasePower") => reckless_bp,
        ("healer", "onResidual") => healer_residual,
        ("owntempo", "onUpdate") => owntempo_update,
        ("owntempo", "onTryAddVolatile") => owntempo_try_add_volatile,
        ("owntempo", "onTryBoost") => intimidate_immune_try_boost,
        ("aromaveil", "onAllyTryAddVolatile") => aromaveil_try_add_volatile,
        ("analytic", "onBasePower") => analytic_bp,
        ("cheekpouch", "onEatItem") => cheekpouch_eat,
        ("sapsipper", "onTryHit") => sapsipper_try_hit,
        ("effectspore", "onDamagingHit") => effectspore_hit,
        ("screencleaner", "onStart") => screencleaner_start,
        ("tangledfeet", "onModifyAccuracy") => tangledfeet_accuracy,
        ("heatproof", "onSourceModifyAtk") | ("heatproof", "onSourceModifySpA") => heatproof_mod,
        ("heatproof", "onDamage") => heatproof_damage,
        ("sweetveil", "onAllySetStatus") => sweetveil_set_status,
        ("sweetveil", "onAllyTryAddVolatile") => sweetveil_try_add_volatile,
        ("curiousmedicine", "onStart") => curiousmedicine_start,
        ("plus", "onModifySpA") | ("minus", "onModifySpA") => plus_spa,
        ("strongjaw", "onBasePower") => strongjaw_bp,
        ("eelevate", "onSourceAfterFaint") => eelevate_after_faint,
        ("wanderingspirit", "onDamagingHit") => wanderingspirit_hit,

        ("supremeoverlord", "onStart") => supremeoverlord_start,
        ("supremeoverlord", "onBasePower") => supremeoverlord_bp,
        ("supremeoverlord", "onEnd") => supremeoverlord_end,
        ("quickdraw", "onFractionalPriority") => quickdraw_priority,
        ("magmaarmor", "onUpdate") => magmaarmor_update,
        ("magmaarmor", "onImmunity") => magmaarmor_immunity,
        ("hustle", "onModifyAtk") => hustle_atk,
        ("hustle", "onSourceModifyAccuracy") => hustle_accuracy,
        ("immunity", "onUpdate") => immunity_update,
        ("immunity", "onSetStatus") => immunity_set_status,
        ("shielddust", "onModifySecondaries") => shielddust_secondaries,
        ("cutecharm", "onDamagingHit") => cutecharm_hit,
        ("rivalry", "onBasePower") => rivalry_bp,
        ("heavymetal", "onModifyWeight") => heavymetal_weight,
        ("lightmetal", "onModifyWeight") => lightmetal_weight,
        ("icebody", "onWeather") => icebody_weather,
        ("icebody", "onImmunity") => icebody_immunity,
        ("forecast", "onStart") => forecast_start,
        ("forecast", "onWeatherChange") => forecast_weather_change,
        ("mimicry", "onStart") => mimicry_start,
        ("mimicry", "onTerrainChange") => mimicry_terrain_change,
        ("motordrive", "onTryHit") => motordrive_try_hit,
        ("purifyingsalt", "onSetStatus") => purifyingsalt_set_status,
        ("purifyingsalt", "onTryAddVolatile") => purifyingsalt_try_add_volatile,
        ("purifyingsalt", "onSourceModifyAtk") => purifyingsalt_weaken,
        ("purifyingsalt", "onSourceModifySpA") => purifyingsalt_weaken,
        ("harvest", "onResidual") => harvest_residual,
        ("cudchew", "onEatItem") => cudchew_eat_item,
        ("opportunist", "onFoeAfterBoost") => opportunist_foe_after_boost,
        ("opportunist", "onAnySwitchIn") => opportunist_flush,
        ("opportunist", "onAnyAfterMega") => opportunist_flush,
        ("opportunist", "onAnyAfterTerastallization") => opportunist_flush,
        ("opportunist", "onAnyAfterMove") => opportunist_flush,
        ("opportunist", "onResidual") => opportunist_flush,
        ("opportunist", "onEnd") => opportunist_end,
        ("cudchew", "onResidual") => cudchew_residual,
        ("sapsipper", "onAllyTryHitSide") => sapsipper_ally_try_hit_side,
        ("merciless", "onModifyCritRatio") => merciless_crit,
        ("sniper", "onModifyDamage") => sniper_damage,

        ("adaptability", "onModifySTAB") => adaptability_stab,
        ("aerilate", "onModifyType") => ate_modify_type,
        ("aerilate", "onBasePower") => ate_base_power,
        ("pixilate", "onModifyType") => ate_modify_type,
        ("pixilate", "onBasePower") => ate_base_power,
        ("refrigerate", "onModifyType") => ate_modify_type,
        ("refrigerate", "onBasePower") => ate_base_power,
        ("dragonize", "onModifyType") => ate_modify_type,
        ("dragonize", "onBasePower") => ate_base_power,
        ("angerpoint", "onHit") => angerpoint_hit,
        ("armortail", "onFoeTryMove") => armortail_foe_try_move,
        ("queenlymajesty", "onFoeTryMove") => armortail_foe_try_move,
        ("dazzling", "onFoeTryMove") => armortail_foe_try_move,
        ("blaze", "onModifyAtk") => pinch_boost,
        ("blaze", "onModifySpA") => pinch_boost,
        ("overgrow", "onModifyAtk") => pinch_boost,
        ("overgrow", "onModifySpA") => pinch_boost,
        ("torrent", "onModifyAtk") => pinch_boost,
        ("torrent", "onModifySpA") => pinch_boost,
        ("swarm", "onModifyAtk") => pinch_boost,
        ("swarm", "onModifySpA") => pinch_boost,
        ("bulletproof", "onTryHit") => bulletproof_try_hit,
        ("chlorophyll", "onModifySpe") => chlorophyll_spe,
        ("swiftswim", "onModifySpe") => swiftswim_spe,
        ("sandrush", "onModifySpe") => sandrush_spe,
        ("sandrush", "onImmunity") => sand_immunity,
        ("sandveil", "onImmunity") => sand_immunity,
        ("sandveil", "onModifyAccuracy") => sandveil_accuracy,
        ("sandforce", "onImmunity") => sand_immunity,
        ("sandforce", "onBasePower") => sandforce_base_power,
        ("snowcloak", "onImmunity") => hail_immunity,
        ("snowcloak", "onModifyAccuracy") => snowcloak_accuracy,
        ("slushrush", "onModifySpe") => slushrush_spe,
        ("surgesurfer", "onModifySpe") => surgesurfer_spe,
        ("clearbody", "onTryBoost") => clearbody_try_boost,
        ("whitesmoke", "onTryBoost") => clearbody_try_boost,
        ("fullmetalbody", "onTryBoost") => clearbody_try_boost,
        ("cloudnine", "onSwitchIn") => cloudnine_switch_in,
        ("cloudnine", "onStart") => cloudnine_start,
        ("cloudnine", "onEnd") => cloudnine_end,
        ("airlock", "onSwitchIn") => cloudnine_switch_in,
        ("airlock", "onStart") => cloudnine_start,
        ("airlock", "onEnd") => cloudnine_end,
        ("competitive", "onAfterEachBoost") => competitive_after_boost,
        ("defiant", "onAfterEachBoost") => defiant_after_boost,
        ("compoundeyes", "onSourceModifyAccuracy") => compoundeyes_accuracy,
        ("contrary", "onChangeBoost") => contrary_change_boost,
        ("simple", "onChangeBoost") => simple_change_boost,
        ("cursedbody", "onDamagingHit") => cursedbody_hit,
        ("damp", "onAnyTryMove") => damp_any_try_move,
        ("damp", "onAnyDamage") => damp_any_damage,
        ("disguise", "onDamage") => disguise_damage,
        ("disguise", "onCriticalHit") => disguise_crit,
        ("disguise", "onEffectiveness") => disguise_effectiveness,
        ("disguise", "onUpdate") => disguise_update,
        ("drizzle", "onStart") => drizzle_start,
        ("drought", "onStart") => drought_start,
        ("sandstream", "onStart") => sandstream_start,
        ("snowwarning", "onStart") => snowwarning_start,
        ("electricsurge", "onStart") => electricsurge_start,
        ("grassysurge", "onStart") => grassysurge_start,
        ("psychicsurge", "onStart") => psychicsurge_start,
        ("mistysurge", "onStart") => mistysurge_start,
        ("seedsower", "onDamagingHit") => seedsower_hit,
        ("electromorphosis", "onDamagingHit") => electromorphosis_hit,
        ("emergencyexit", "onEmergencyExit") => emergencyexit,
        ("wimpout", "onEmergencyExit") => wimpout,
        ("flamebody", "onDamagingHit") => flamebody_hit,
        ("poisonpoint", "onDamagingHit") => poisonpoint_hit,
        ("static", "onDamagingHit") => static_hit,
        ("poisontouch", "onSourceDamagingHit") => poisontouch_hit,
        ("spicyspray", "onDamagingHit") => spicyspray_hit,
        ("flashfire", "onTryHit") => flashfire_try_hit,
        ("flashfire", "onEnd") => flashfire_end,
        ("flowerveil", "onAllyTryBoost") => flowerveil_try_boost,
        ("flowerveil", "onAllySetStatus") => flowerveil_set_status,
        ("flowerveil", "onAllyTryAddVolatile") => flowerveil_try_add_volatile,
        ("friendguard", "onAnyModifyDamage") => friendguard_damage,
        ("frisk", "onStart") => frisk_start,
        ("furcoat", "onModifyDef") => chain2,
        ("hugepower", "onModifyAtk") => chain2,
        ("purepower", "onModifyAtk") => chain2,
        ("galewings", "onModifyPriority") => galewings_priority,
        ("goodasgold", "onTryHit") => goodasgold_try_hit,
        ("grasspelt", "onModifyDef") => grasspelt_def,
        ("hospitality", "onStart") => hospitality_start,
        ("hypercutter", "onTryBoost") => hypercutter_try_boost,
        ("illusion", "onBeforeSwitchIn") => illusion_before_switch_in,
        ("illusion", "onDamagingHit") => illusion_damaging_hit,
        ("illusion", "onEnd") => illusion_end,
        ("illusion", "onFaint") => illusion_faint,
        ("infiltrator", "onModifyMove") => infiltrator_modify_move,
        ("innerfocus", "onTryAddVolatile") => innerfocus_try_add_volatile,
        ("innerfocus", "onTryBoost") => intimidate_immune_try_boost,
        ("guarddog", "onTryBoost") => guarddog_try_boost,
        ("guarddog", "onDragOut") => guarddog_drag_out,
        ("oblivious", "onTryBoost") => intimidate_immune_try_boost,
        ("owntempo", "onTryBoost") => intimidate_immune_try_boost,
        ("scrappy", "onTryBoost") => intimidate_immune_try_boost,
        ("scrappy", "onModifyMove") => scrappy_modify_move,
        ("intimidate", "onStart") => intimidate_start,
        ("ironfist", "onBasePower") => ironfist_base_power,
        ("justified", "onDamagingHit") => justified_hit,
        ("steadfast", "onFlinch") => steadfast_flinch,
        ("liquidooze", "onSourceTryHeal") => liquidooze_try_heal,
        ("libero", "onPrepareHit") => protean_prepare_hit,
        ("protean", "onPrepareHit") => protean_prepare_hit,
        ("lightningrod", "onTryHit") => lightningrod_try_hit,
        ("lightningrod", "onAnyRedirectTarget") => lightningrod_redirect,
        ("stormdrain", "onTryHit") => stormdrain_try_hit,
        ("stormdrain", "onAnyRedirectTarget") => stormdrain_redirect,
        ("limber", "onUpdate") => limber_update,
        ("limber", "onSetStatus") => limber_set_status,
        ("liquidvoice", "onModifyType") => liquidvoice_modify_type,
        ("magicbounce", "onTryHit") => magicbounce_try_hit,
        ("magicbounce", "onAllyTryHitSide") => magicbounce_ally_try_hit_side,
        ("magicguard", "onDamage") => magicguard_damage,
        ("megalauncher", "onBasePower") => megalauncher_base_power,
        ("mirrorarmor", "onTryBoost") => mirrorarmor_try_boost,
        ("moldbreaker", "onStart") => moldbreaker_start,
        ("moldbreaker", "onModifyMove") => moldbreaker_modify_move,
        ("moody", "onResidual") => moody_residual,
        ("moxie", "onSourceAfterFaint") => moxie_after_faint,
        ("multiscale", "onSourceModifyDamage") => multiscale_damage,
        ("naturalcure", "onSwitchOut") => naturalcure_switch_out,
        ("oblivious", "onUpdate") => oblivious_update,
        ("oblivious", "onImmunity") => oblivious_immunity,
        ("oblivious", "onTryHit") => oblivious_try_hit,
        ("pickpocket", "onAfterMoveSecondary") => pickpocket_after,
        ("prankster", "onModifyPriority") => prankster_priority,
        ("pressure", "onStart") => pressure_start,
        ("pressure", "onDeductPP") => pressure_deduct_pp,
        ("punkrock", "onBasePower") => punkrock_base_power,
        ("punkrock", "onSourceModifyDamage") => punkrock_damage,
        ("raindish", "onWeather") => raindish_weather,
        ("regenerator", "onSwitchOut") => regenerator_switch_out,
        ("rockhead", "onDamage") => rockhead_damage,
        ("roughskin", "onDamagingHit") => roughskin_hit,
        ("ironbarbs", "onDamagingHit") => roughskin_hit,
        ("sharpness", "onBasePower") => sharpness_base_power,
        ("shadowtag", "onFoeTrapPokemon") => shadowtag_trap,
        ("shadowtag", "onFoeMaybeTrapPokemon") => shadowtag_maybe_trap,
        ("solarpower", "onModifySpA") => solarpower_spa,
        ("solarpower", "onWeather") => solarpower_weather,
        ("solidrock", "onSourceModifyDamage") => solidrock_damage,
        ("filter", "onSourceModifyDamage") => solidrock_damage,
        ("prismarmor", "onSourceModifyDamage") => solidrock_damage,
        ("soundproof", "onTryHit") => soundproof_try_hit,
        ("soundproof", "onAllyTryHitSide") => soundproof_ally_try_hit_side,
        ("speedboost", "onResidual") => speedboost_residual,
        ("stalwart", "onModifyMove") => stalwart_modify_move,
        ("stamina", "onDamagingHit") => stamina_hit,
        ("stancechange", "onModifyMove") => stancechange_modify_move,
        ("steelyspirit", "onAllyBasePower") => steelyspirit_base_power,
        ("sturdy", "onTryHit") => sturdy_try_hit,
        ("sturdy", "onDamage") => sturdy_damage,
        ("superluck", "onModifyCritRatio") => superluck_crit,
        ("symbiosis", "onAllyAfterUseItem") => symbiosis_after_use_item,
        ("synchronize", "onAfterSetStatus") => synchronize_after_set_status,
        ("technician", "onBasePower") => technician_base_power,
        ("telepathy", "onTryHit") => telepathy_try_hit,
        ("thermalexchange", "onDamagingHit") => thermalexchange_hit,
        ("thermalexchange", "onUpdate") => thermalexchange_update,
        ("thermalexchange", "onSetStatus") => thermalexchange_set_status,
        ("thickfat", "onSourceModifyAtk") => thickfat_mod,
        ("thickfat", "onSourceModifySpA") => thickfat_mod,
        ("toughclaws", "onBasePower") => toughclaws_base_power,
        ("toxicdebris", "onDamagingHit") => toxicdebris_hit,
        ("trace", "onStart") => trace_start,
        ("trace", "onUpdate") => trace_update,
        ("unaware", "onAnyModifyBoost") => unaware_modify_boost,
        ("unburden", "onAfterUseItem") => unburden_after_use_item,
        ("unburden", "onTakeItem") => unburden_take_item,
        ("unburden", "onEnd") => unburden_end,
        ("unnerve", "onStart") => unnerve_start,
        ("unnerve", "onEnd") => unnerve_end,
        ("unnerve", "onFoeTryEatItem") => unnerve_foe_try_eat,
        ("voltabsorb", "onTryHit") => voltabsorb_try_hit,
        ("waterabsorb", "onTryHit") => waterabsorb_try_hit,
        ("weakarmor", "onDamagingHit") => weakarmor_hit,
        ("fairyaura", "onStart") => fairyaura_start,
        ("fairyaura", "onAnyBasePower") => fairyaura_base_power,
        ("auraguard", "onSourceModifyDamage") => auraguard_damage,
        ("noguard", "onAnyInvulnerability") => noguard_invulnerability,
        ("noguard", "onAnyAccuracy") => noguard_accuracy,
        ("sheerforce", "onModifyMove") => sheerforce_modify_move,
        ("sheerforce", "onBasePower") => sheerforce_base_power,
        ("firemane", "onModifyAtk") => firemane_mod,
        ("firemane", "onModifySpA") => firemane_mod,
        ("megasol", "onWeatherModifyDamage") => megasol_weather_damage,
        ("parentalbond", "onPrepareHit") => parentalbond_prepare_hit,
        ("parentalbond", "onSourceModifySecondaries") => noop,
        _ => return None,
    };
    Some(f)
}

fn noop(_: &mut Battle, _: &HArgs) -> Val {
    Val::Undef
}
fn chain2(b: &mut Battle, _: &HArgs) -> Val {
    b.chain_modify(2.0, 1.0);
    Val::Undef
}
fn mtype(b: &Battle, mv: u16) -> TypeId {
    b.mv(mv).type_
}
fn is_type(b: &Battle, mv: u16, t: &str) -> bool {
    b.mv(mv).type_ == b.d.type_id(t)
}
fn this_holder(a: &HArgs) -> Mon {
    holder(a)
}

// ------------------------------------------------------------------
fn adaptability_stab(b: &mut Battle, a: &HArgs) -> Val {
    // onModifySTAB(stab, source, target, move)
    let s = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let t = mtype(b, mv);
    if b.mv(mv).force_stab || b.has_type_id(s, t) {
        if a.relay.num() == 2.0 {
            return Val::Num(2.25);
        }
        return Val::Num(2.0);
    }
    Val::Undef
}
fn ate_modify_type(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    let to = match b.eff(a.this_eff).id.as_str() {
        "aerilate" => "Flying",
        "pixilate" => "Fairy",
        "refrigerate" => "Ice",
        "dragonize" => "Dragon",
        _ => return Val::Undef,
    };
    let id = b.move_def(mv).id.as_str();
    let no = ["judgment", "multiattack", "naturalgift", "revelationdance", "technoblast", "terrainpulse", "weatherball"];
    if is_type(b, mv, "Normal") && !no.contains(&id) {
        let t = b.d.type_id(to);
        b.mvm(mv).type_ = t;
        b.mvm(mv).type_changer_boosted = Some(a.this_eff);
    }
    Val::Undef
}
fn ate_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).type_changer_boosted == Some(a.this_eff) {
        b.chain_modify(4915.0, 4096.0);
    }
    Val::Undef
}
fn angerpoint_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).hp == 0 {
        return Val::Undef;
    }
    if let Some(mv) = amv(a) {
        if b.hit_data(mv, t).crit {
            let bo = Battle::boosts1(B_ATK, 12);
            b.boost(bo, Some(t), Some(t), EffRef::NONE, false, false);
        }
    }
    Val::Undef
}
fn armortail_foe_try_move(b: &mut Battle, a: &HArgs) -> Val {
    // onFoeTryMove(target, source, move): target = the move's user, source = its target
    let user = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let mt = b.mv(mv).target;
    let id = b.move_def(mv).id.as_str();
    if mt == MoveTarget::FoeSide || (mt == MoveTarget::All && !["perishsong", "flowershield", "rototiller"].contains(&id)) {
        return Val::Undef;
    }
    let h = this_holder(a);
    let source_ally = src(a).map(|s| s.s == h.s).unwrap_or(false);
    if (source_ally || mt == MoveTarget::All) && b.mv(mv).priority > 0.1 {
        b.attr_last_move(&["[still]"]);
        let an = b.eff(a.this_eff).name.clone();
        let mn = b.move_def(mv).name.clone();
        let us = b.mon_str(user);
        add!(b, "cant", h, format!("ability: {an}"), mn, format!("[of] {us}"));
        return Val::False;
    }
    Val::Undef
}
fn pinch_boost(b: &mut Battle, a: &HArgs) -> Val {
    let att = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let t = match b.eff(a.this_eff).id.as_str() {
        "blaze" => "Fire",
        "overgrow" => "Grass",
        "torrent" => "Water",
        _ => "Bug",
    };
    if is_type(b, mv, t) && b.p(att).hp as f64 <= b.p(att).maxhp as f64 / 3.0 {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn bulletproof_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::BULLET != 0 {
        add!(b, "-immune", t, "[from] ability: Bulletproof");
        return Val::Null;
    }
    Val::Undef
}
fn chlorophyll_spe(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if matches!(b.effective_weather(m, EffRef::NONE, false), "sunnyday" | "desolateland") {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn swiftswim_spe(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if matches!(b.effective_weather(m, EffRef::NONE, false), "raindance" | "primordialsea") {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn sandrush_spe(b: &mut Battle, _: &HArgs) -> Val {
    if b.is_weather(&["sandstorm"]) {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn slushrush_spe(b: &mut Battle, _: &HArgs) -> Val {
    if b.is_weather(&["hail", "snowscape"]) {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn surgesurfer_spe(b: &mut Battle, _: &HArgs) -> Val {
    if b.is_terrain("electricterrain", None) {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn sand_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("sandstorm") {
        return Val::False;
    }
    Val::Undef
}
fn hail_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("hail") {
        return Val::False;
    }
    Val::Undef
}
fn sandveil_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    if b.is_weather(&["sandstorm"]) {
        b.chain_modify(3277.0, 4096.0);
    }
    Val::Undef
}
fn snowcloak_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    if b.is_weather(&["hail", "snowscape"]) {
        b.chain_modify(3277.0, 4096.0);
    }
    Val::Undef
}
fn sandforce_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.is_weather(&["sandstorm"]) && (is_type(b, mv, "Rock") || is_type(b, mv, "Ground") || is_type(b, mv, "Steel")) {
        b.chain_modify(5325.0, 4096.0);
    }
    Val::Undef
}
/// effect.secondaries (the effect that asked for the boost)
fn effect_has_secondaries(b: &Battle, e: EffRef) -> bool {
    if e.is_move() {
        return b.mv(e.mv).secondaries.is_some();
    }
    false
}
fn clearbody_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a) == Some(t) {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    let mut show = false;
    for (k, v) in bo.clone().iter() {
        if v < 0 {
            bo.remove(k);
            show = true;
        }
    }
    if show && !effect_has_secondaries(b, a.effect) && b.eid(a.effect) != "octolock" {
        let an = b.eff(a.this_eff).name.clone();
        let ts = b.mon_str(t);
        add!(b, "-fail", t, "unboost", format!("[from] ability: {an}"), format!("[of] {ts}"));
    }
    Val::Boosts(bo)
}
fn cloudnine_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let n = b.eff(a.this_eff).name.clone();
    add!(b, "-ability", m, n);
    cloudnine_start(b, a)
}
fn cloudnine_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).ability_state.set(K::Ending, Val::False);
    b.each_event(Ev::WeatherChange, EffRef::eff(a.this_eff));
    Val::Undef
}
fn cloudnine_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).ability_state.set(K::Ending, Val::True);
    b.each_event(Ev::WeatherChange, EffRef::eff(a.this_eff));
    Val::Undef
}
fn stat_lowered_by_foe(a: &HArgs) -> Option<Mon> {
    let t = tgt(a);
    let s = src(a)?;
    if t.s == s.s {
        return None;
    }
    if let Val::Boosts(bo) = a.relay {
        if bo.iter().any(|(_, v)| v < 0) {
            return Some(t);
        }
    }
    None
}
fn competitive_after_boost(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(t) = stat_lowered_by_foe(a) {
        let bo = Battle::boosts1(B_SPA, 2);
        b.boost(bo, Some(t), Some(t), EffRef::NONE, false, true);
    }
    Val::Undef
}
fn defiant_after_boost(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(t) = stat_lowered_by_foe(a) {
        let bo = Battle::boosts1(B_ATK, 2);
        b.boost(bo, Some(t), Some(t), EffRef::NONE, false, true);
    }
    Val::Undef
}
fn compoundeyes_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    b.chain_modify(5325.0, 4096.0);
    Val::Undef
}
fn contrary_change_boost(b: &mut Battle, a: &HArgs) -> Val {
    if b.eid(a.effect) == "zpower" {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    for k in 0..bo.n as usize {
        bo.e[k].1 = -bo.e[k].1;
    }
    Val::Boosts(bo)
}
fn simple_change_boost(b: &mut Battle, a: &HArgs) -> Val {
    if b.eid(a.effect) == "zpower" {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    for k in 0..bo.n as usize {
        bo.e[k].1 *= 2;
    }
    Val::Boosts(bo)
}
fn cursedbody_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(s) = src(a) else { return Val::Undef };
    if b.has_vol(s, "disable") {
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::FUTUREMOVE == 0 && b.move_def(mv).id != "struggle" && b.random_chance(3, 10) {
        let dis = b.d.cond("disable").unwrap();
        let h = this_holder(a);
        b.add_volatile(s, dis, Some(h), EffRef::NONE, None);
    }
    Val::Undef
}
fn damp_any_try_move(b: &mut Battle, a: &HArgs) -> Val {
    let user = tgt(a);
    let id = b.eid(a.effect);
    if matches!(id, "explosion" | "mindblown" | "mistyexplosion" | "selfdestruct") {
        b.attr_last_move(&["[still]"]);
        let h = this_holder(a);
        let n = b.ename(a.effect).to_string();
        let us = b.mon_str(user);
        add!(b, "cant", h, "ability: Damp", n, format!("[of] {us}"));
        return Val::False;
    }
    Val::Undef
}
fn damp_any_damage(b: &mut Battle, a: &HArgs) -> Val {
    if b.ename(a.effect) == "Aftermath" {
        return Val::False;
    }
    Val::Undef
}
fn is_mimikyu(b: &Battle, m: Mon) -> bool {
    matches!(b.d.species[b.p(m).species as usize].id.as_str(), "mimikyu" | "mimikyutotem")
}
fn disguise_damage(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.etype(a.effect) == EffType::Move && is_mimikyu(b, t) {
        add!(b, "-activate", t, "ability: Disguise");
        b.sset(a.state, K::Busted, Val::True);
        return Val::Num(0.0);
    }
    Val::Undef
}
fn disguise_hit_sub(b: &Battle, t: Mon, mv: u16) -> bool {
    b.has_volatile(t, b.d.c.substitute) && b.mv(mv).flags & mflag::BYPASSSUB == 0 && !b.mv(mv).infiltrates
}
fn disguise_crit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if !is_mimikyu(b, t) {
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    if disguise_hit_sub(b, t, mv) {
        return Val::Undef;
    }
    if !b.run_immunity_move(t, mv, false) {
        return Val::Undef;
    }
    Val::False
}
fn disguise_effectiveness(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Status || !is_mimikyu(b, t) {
        return Val::Undef;
    }
    if disguise_hit_sub(b, t, mv) {
        return Val::Undef;
    }
    if !b.run_immunity_move(t, mv, false) {
        return Val::Undef;
    }
    Val::Num(0.0)
}
fn disguise_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if is_mimikyu(b, m) && b.sget(a.state, K::Busted).truthy() {
        let totem = b.d.species[b.p(m).species as usize].id == "mimikyutotem";
        let sp = b.d.species_id(if totem { "Mimikyu-Busted-Totem" } else { "Mimikyu-Busted" }).unwrap() as u16;
        b.forme_change(m, sp, EffRef::eff(a.this_eff), true, None);
        let d = b.p(m).base_maxhp as f64 / 8.0;
        // the damage's effect is the busted species: "[from] pokemon: Mimikyu-Busted"
        let sp_eff = b.cond(if totem { "species:mimikyubustedtotem" } else { "species:mimikyubusted" });
        b.damage(d, Some(m), Some(m), EffRef::eff(sp_eff), false);
    }
    Val::Undef
}
fn weather_setter(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let m = tgt(a);
    let w = b.cond(id);
    b.set_weather(w, Some(m), EffRef::eff(a.this_eff));
    Val::Undef
}
fn drizzle_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.d.species[b.p(m).species as usize].id == "kyogre" && b.item_id(m) == "blueorb" {
        return Val::Undef;
    }
    weather_setter(b, a, "raindance")
}
fn drought_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.d.species[b.p(m).species as usize].id == "groudon" && b.item_id(m) == "redorb" {
        return Val::Undef;
    }
    weather_setter(b, a, "sunnyday")
}
fn sandstream_start(b: &mut Battle, a: &HArgs) -> Val {
    weather_setter(b, a, "sandstorm")
}
fn snowwarning_start(b: &mut Battle, a: &HArgs) -> Val {
    weather_setter(b, a, "snowscape")
}
fn terrain_setter(b: &mut Battle, a: &HArgs, id: &str) -> Val {
    let m = tgt(a);
    let t = b.cond(id);
    b.set_terrain(t, Some(m), EffRef::eff(a.this_eff));
    Val::Undef
}
fn electricsurge_start(b: &mut Battle, a: &HArgs) -> Val {
    terrain_setter(b, a, "electricterrain")
}
fn grassysurge_start(b: &mut Battle, a: &HArgs) -> Val {
    terrain_setter(b, a, "grassyterrain")
}
fn psychicsurge_start(b: &mut Battle, a: &HArgs) -> Val {
    terrain_setter(b, a, "psychicterrain")
}
fn mistysurge_start(b: &mut Battle, a: &HArgs) -> Val {
    terrain_setter(b, a, "mistyterrain")
}
fn seedsower_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = b.cond("grassyterrain");
    // setTerrain without a source: source = event.target (the damaged holder)
    b.set_terrain(t, None, EffRef::eff(a.this_eff));
    Val::Undef
}
fn electromorphosis_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let c = b.cond("charge");
    b.add_volatile(t, c, None, EffRef::NONE, None);
    Val::Undef
}
fn emergencyexit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.can_switch(t.s as usize) == 0 || b.p(t).force_switch_flag || b.p(t).switch_flag.is_set() {
        return Val::Undef;
    }
    b.pm(t).switch_flag = SwitchFlag::Yes;
    add!(b, "-activate", t, "ability: Emergency Exit");
    Val::Undef
}
fn wimpout(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.can_switch(t.s as usize) == 0 || b.p(t).force_switch_flag || b.p(t).switch_flag.is_set() {
        return Val::Undef;
    }
    b.pm(t).switch_flag = SwitchFlag::Yes;
    add!(b, "-activate", t, "ability: Wimp Out");
    Val::Undef
}
fn contact_status(b: &mut Battle, a: &HArgs, status: &str) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) && b.random_chance(3, 10) {
        let st = b.cond(status);
        b.try_set_status(s, st, Some(t), EffRef::NONE);
    }
    Val::Undef
}
fn flamebody_hit(b: &mut Battle, a: &HArgs) -> Val {
    contact_status(b, a, "brn")
}
fn poisonpoint_hit(b: &mut Battle, a: &HArgs) -> Val {
    contact_status(b, a, "psn")
}
fn static_hit(b: &mut Battle, a: &HArgs) -> Val {
    contact_status(b, a, "par")
}
fn poisontouch_hit(b: &mut Battle, a: &HArgs) -> Val {
    // onSourceDamagingHit(damage, target, source, move): source = the holder
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    if b.has_ability(target, "shielddust") || b.has_item(target, "covertcloak") {
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, target, source, false) && b.random_chance(3, 10) {
        let psn = b.d.c.psn;
        b.try_set_status(target, psn, Some(source), EffRef::NONE);
    }
    Val::Undef
}
fn spicyspray_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let brn = b.d.c.brn;
    b.try_set_status(s, brn, Some(t), EffRef::NONE);
    Val::Undef
}
fn flashfire_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, "Fire") {
        b.mvm(mv).accuracy = None;
        let ff = b.cond("flashfire");
        if !b.add_volatile(t, ff, None, EffRef::NONE, None).truthy() {
            add!(b, "-immune", t, "[from] ability: Flash Fire");
        }
        return Val::Null;
    }
    Val::Undef
}
fn flashfire_end(b: &mut Battle, a: &HArgs) -> Val {
    b.remove_volatile_id(tgt(a), "flashfire");
    Val::Undef
}
fn flowerveil_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a) == Some(t) || !b.has_type(t, "Grass") {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    let mut show = false;
    for (k, v) in bo.clone().iter() {
        if v < 0 {
            bo.remove(k);
            show = true;
        }
    }
    if show && !effect_has_secondaries(b, a.effect) {
        let h = this_holder(a);
        let hs = b.mon_str(h);
        add!(b, "-block", t, "ability: Flower Veil", format!("[of] {hs}"));
    }
    Val::Boosts(bo)
}
fn flowerveil_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let s = src(a);
    if b.has_type(t, "Grass") && s.is_some() && s != Some(t) && !a.effect.is_none() && b.eid(a.effect) != "yawn" {
        let et = b.etype(a.effect);
        if b.ename(a.effect) == "Synchronize" || (et == EffType::Move && !effect_has_secondaries(b, a.effect)) {
            let h = this_holder(a);
            let hs = b.mon_str(h);
            add!(b, "-block", t, "ability: Flower Veil", format!("[of] {hs}"));
        }
        return Val::Null;
    }
    Val::Undef
}
fn flowerveil_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Val::Eff(e) = a.relay {
        if b.has_type(t, "Grass") && b.eff(e).id == "yawn" {
            let h = this_holder(a);
            let hs = b.mon_str(h);
            add!(b, "-block", t, "ability: Flower Veil", format!("[of] {hs}"));
            return Val::Null;
        }
    }
    Val::Undef
}
fn friendguard_damage(b: &mut Battle, a: &HArgs) -> Val {
    // onAnyModifyDamage(damage, source, target, move): target = the defender (event source)
    let h = this_holder(a);
    let Some(def) = src(a) else { return Val::Undef };
    if def != h && def.s == h.s {
        b.chain_modify(0.75, 1.0);
    }
    Val::Undef
}
fn frisk_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for f in b.foes_of(m.s) {
        if b.p(f).item != b.d.c.empty {
            let n = b.eff(b.p(f).item).name.clone();
            let ms = b.mon_str(m);
            add!(b, "-item", f, n, "[from] ability: Frisk", format!("[of] {ms}"));
        }
    }
    Val::Undef
}
fn galewings_priority(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if is_type(b, mv, "Flying") && b.p(m).hp == b.p(m).maxhp {
        return Val::Num(a.relay.num() + 1.0);
    }
    Val::Undef
}
fn goodasgold_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Status && src(a) != Some(t) {
        add!(b, "-immune", t, "[from] ability: Good as Gold");
        return Val::Null;
    }
    Val::Undef
}
fn grasspelt_def(b: &mut Battle, _: &HArgs) -> Val {
    if b.is_terrain("grassyterrain", None) {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn hospitality_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for ally in b.adjacent_allies(m) {
        let h = b.p(ally).base_maxhp as f64 / 4.0;
        b.heal(h, Some(ally), Some(m), EffRef::NONE);
    }
    Val::Undef
}
fn hypercutter_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a) == Some(t) {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if let Some(v) = bo.get(B_ATK) {
        if v < 0 {
            bo.remove(B_ATK);
            if !effect_has_secondaries(b, a.effect) {
                let ts = b.mon_str(t);
                add!(b, "-fail", t, "unboost", "atk", "[from] ability: Hyper Cutter", format!("[of] {ts}"));
            }
        }
    }
    Val::Boosts(bo)
}
fn infiltrator_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        b.mvm(mv).infiltrates = true;
    }
    Val::Undef
}
fn innerfocus_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Eff(b.d.c.flinch) {
        return Val::Null;
    }
    Val::Undef
}
fn intimidate_immune_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if b.ename(a.effect) == "Intimidate" && bo.get(B_ATK).map(|v| v != 0).unwrap_or(false) {
        bo.remove(B_ATK);
        let an = b.eff(a.this_eff).name.clone();
        let ts = b.mon_str(t);
        add!(b, "-fail", t, "unboost", "atk", format!("[from] ability: {an}"), format!("[of] {ts}"));
        return Val::Boosts(bo);
    }
    Val::Undef
}
// Guard Dog: Intimidate raises its Attack instead; it can't be forced out (Roar, Dragon Tail...)
fn guarddog_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if b.ename(a.effect) == "Intimidate" && bo.get(B_ATK).map(|v| v != 0).unwrap_or(false) {
        bo.remove(B_ATK);
        let mut up = SparseBoosts::default();
        up.set(B_ATK, 1);
        b.boost(up, Some(t), Some(t), EffRef::NONE, false, true);
        return Val::Boosts(bo);
    }
    Val::Undef
}
fn guarddog_drag_out(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    add!(b, "-activate", t, "ability: Guard Dog");
    Val::Null
}
fn scrappy_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    let fighting = b.d.type_id("Fighting");
    let normal = b.d.type_id("Normal");
    let cur = b.mv(mv).ignore_immunity;
    let new = match cur {
        Some(IgnoreImmunity::All) => IgnoreImmunity::All,
        Some(IgnoreImmunity::Types(bits)) => IgnoreImmunity::Types(bits | (1 << fighting) | (1 << normal)),
        _ => IgnoreImmunity::Types((1 << fighting) | (1 << normal)),
    };
    b.mvm(mv).ignore_immunity = Some(new);
    Val::Undef
}
fn intimidate_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let mut activated = false;
    for f in b.adjacent_foes(m) {
        if !activated {
            add!(b, "-ability", m, "Intimidate", "boost");
            activated = true;
        }
        if b.has_volatile(f, b.d.c.substitute) {
            add!(b, "-immune", f);
        } else {
            let bo = Battle::boosts1(B_ATK, -1);
            b.boost(bo, Some(f), Some(m), EffRef::NONE, true, false);
        }
    }
    Val::Undef
}
fn ironfist_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::PUNCH != 0 {
            b.chain_modify(4915.0, 4096.0);
        }
    }
    Val::Undef
}
fn steadfast_flinch(b: &mut Battle, _a: &HArgs) -> Val {
    let bo = Battle::boosts1(B_SPE, 1);
    b.boost(bo, None, None, EffRef::NONE, false, false);
    Val::Undef
}
/// Showdown onSourceTryHeal: drain / Leech Seed / Strength Sap healing turns into damage to the healer
fn liquidooze_try_heal(b: &mut Battle, a: &HArgs) -> Val {
    if a.effect.is_none() {
        return Val::Undef;
    }
    if matches!(b.eff(a.effect.eff).id.as_str(), "drain" | "leechseed" | "strengthsap") {
        let d = a.relay.num();
        b.damage(d, a.target.mon(), a.source.mon(), EffRef::eff(a.this_eff), false);
        return Val::Num(0.0);
    }
    Val::Undef
}
fn justified_hit(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Dark") {
            let bo = Battle::boosts1(B_ATK, 1);
            b.boost(bo, None, None, EffRef::NONE, false, false);
        }
    }
    Val::Undef
}
fn protean_prepare_hit(b: &mut Battle, a: &HArgs) -> Val {
    let source = tgt(a);
    let key = if b.eff(a.this_eff).id == "libero" { K::Libero } else { K::X3 };
    if b.sget(a.state, key).truthy() {
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    let m = b.mv(mv);
    if m.has_bounced || m.flags & mflag::FUTUREMOVE != 0 || b.move_def(mv).calls_move {
        return Val::Undef;
    }
    let t = m.type_;
    if t == NO_TYPE {
        return Val::Undef;
    }
    let types = b.get_types(source, false);
    if !(types.len() == 1 && types[0] == t) {
        b.pm(source).types = vec![t];
        b.pm(source).added_type = NO_TYPE;
        b.pm(source).known_type = true;
        let tn = b.d.type_name(t).to_string();
        b.pm(source).apparent_type = tn.clone();
        b.sset(a.state, key, Val::True);
        let an = b.eff(a.this_eff).name.clone();
        add!(b, "-start", source, "typechange", tn, format!("[from] ability: {an}"));
    }
    Val::Undef
}
fn lightningrod_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, "Electric") {
        let bo = Battle::boosts1(B_SPA, 1);
        if !b.boost(bo, None, None, EffRef::NONE, false, false).truthy() {
            add!(b, "-immune", t, "[from] ability: Lightning Rod");
        }
        return Val::Null;
    }
    Val::Undef
}
fn absorb_redirect(b: &mut Battle, a: &HArgs, ty: &str, msg: &str) -> Val {
    // onAnyRedirectTarget(target, source, source2, move): relay = current target, source = the user
    let Some(mv) = amv(a) else { return Val::Undef };
    if !is_type(b, mv, ty) || b.mv(mv).flags & mflag::bit("pledgecombo") != 0 {
        return Val::Undef;
    }
    let mt = b.mv(mv).target;
    let rt = if matches!(mt, MoveTarget::RandomNormal | MoveTarget::AdjacentFoe) { MoveTarget::Normal } else { mt };
    let user = tgt(a);
    let h = this_holder(a);
    if b.valid_target(h, user, rt) {
        if b.mv(mv).smart_target == Some(true) {
            b.mvm(mv).smart_target = Some(false);
        }
        if a.relay != Val::Mon(h) {
            add!(b, "-activate", h, msg);
        }
        return Val::Mon(h);
    }
    Val::Undef
}
fn lightningrod_redirect(b: &mut Battle, a: &HArgs) -> Val {
    absorb_redirect(b, a, "Electric", "ability: Lightning Rod")
}
fn stormdrain_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, "Water") {
        let bo = Battle::boosts1(B_SPA, 1);
        if !b.boost(bo, None, None, EffRef::NONE, false, false).truthy() {
            add!(b, "-immune", t, "[from] ability: Storm Drain");
        }
        return Val::Null;
    }
    Val::Undef
}
fn stormdrain_redirect(b: &mut Battle, a: &HArgs) -> Val {
    absorb_redirect(b, a, "Water", "ability: Storm Drain")
}
fn limber_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.par {
        add!(b, "-activate", m, "ability: Limber");
        b.cure_status(m, false);
    }
    Val::Undef
}
fn limber_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay != Val::Eff(b.d.c.par) {
        return Val::Undef;
    }
    if b.move_status_of(a.effect).is_some() {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Limber");
    }
    Val::False
}
fn liquidvoice_modify_type(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::SOUND != 0 && !b.has_vol(m, "dynamax") {
        let w = b.d.type_id("Water");
        b.mvm(mv).type_ = w;
    }
    Val::Undef
}
fn magicbounce_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if t == s || b.mv(mv).has_bounced || b.mv(mv).flags & mflag::REFLECTABLE == 0 || b.is_semi_invulnerable(t) {
        return Val::Undef;
    }
    let nm = b.new_active_move(b.mv(mv).def);
    b.mvm(nm).has_bounced = true;
    b.mvm(nm).prankster_boosted = false;
    b.use_move(nm, t, Some(s), None);
    Val::Null
}
fn magicbounce_ally_try_hit_side(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if t.s == s.s || b.mv(mv).has_bounced || b.mv(mv).flags & mflag::REFLECTABLE == 0 || b.is_semi_invulnerable(t) {
        return Val::Undef;
    }
    let nm = b.new_active_move(b.mv(mv).def);
    b.mvm(nm).has_bounced = true;
    b.mvm(nm).prankster_boosted = false;
    let h = this_holder(a);
    b.use_move(nm, h, Some(s), None);
    b.mvm(mv).has_bounced = true;
    Val::Null
}
fn magicguard_damage(b: &mut Battle, a: &HArgs) -> Val {
    if b.etype(a.effect) != EffType::Move {
        if b.etype(a.effect) == EffType::Ability {
            if let Some(s) = src(a) {
                let n = b.ename(a.effect).to_string();
                add!(b, "-activate", s, format!("ability: {n}"));
            }
        }
        return Val::False;
    }
    Val::Undef
}
fn megalauncher_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::PULSE != 0 {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn mirrorarmor_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if t == s || b.ename(a.effect) == "Mirror Armor" {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    for (k, v) in bo.clone().iter() {
        if v < 0 {
            if b.p(t).boosts[k as usize] == -6 {
                continue;
            }
            let neg = Battle::boosts1(k, v);
            bo.remove(k);
            if b.p(s).hp > 0 {
                add!(b, "-ability", t, "Mirror Armor");
                b.boost(neg, Some(s), Some(t), EffRef::NONE, true, false);
            }
        }
    }
    Val::Boosts(bo)
}
fn moldbreaker_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-ability", m, "Mold Breaker");
    Val::Undef
}
fn moldbreaker_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        b.mvm(mv).ignore_ability = true;
    }
    Val::Undef
}
fn moody_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let mut stats: Vec<u8> = (0..5u8).filter(|&k| b.p(m).boosts[k as usize] < 6).collect();
    let mut bo = SparseBoosts::default();
    let plus = if stats.is_empty() { None } else { Some(stats[b.prng.sample_index(stats.len())]) };
    if let Some(p) = plus {
        bo.set(p, 2);
    }
    stats = (0..5u8).filter(|&k| b.p(m).boosts[k as usize] > -6 && Some(k) != plus).collect();
    let minus = if stats.is_empty() { None } else { Some(stats[b.prng.sample_index(stats.len())]) };
    if let Some(mi) = minus {
        bo.set(mi, -1);
    }
    b.boost(bo, Some(m), Some(m), EffRef::NONE, false, false);
    Val::Undef
}
fn moxie_after_faint(b: &mut Battle, a: &HArgs) -> Val {
    if b.etype(a.effect) == EffType::Move {
        let bo = Battle::boosts1(B_ATK, a.relay.num() as i8);
        b.boost(bo, src(a), None, EffRef::NONE, false, false);
    }
    Val::Undef
}
fn multiscale_damage(b: &mut Battle, a: &HArgs) -> Val {
    let Some(def) = src(a) else { return Val::Undef };
    if b.p(def).hp >= b.p(def).maxhp {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn naturalcure_switch_out(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.empty {
        return Val::Undef;
    }
    let sid = b.eff(b.p(m).status).id.clone();
    add!(b, "-curestatus", m, sid, "[from] ability: Natural Cure", "[silent]");
    b.clear_status(m);
    Val::Undef
}
fn oblivious_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_vol(m, "attract") {
        add!(b, "-activate", m, "ability: Oblivious");
        b.remove_volatile_id(m, "attract");
        add!(b, "-end", m, "move: Attract", "[from] ability: Oblivious");
    }
    if b.has_vol(m, "taunt") {
        add!(b, "-activate", m, "ability: Oblivious");
        b.remove_volatile_id(m, "taunt");
    }
    Val::Undef
}
fn oblivious_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("attract") {
        return Val::False;
    }
    Val::Undef
}
fn oblivious_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if matches!(b.move_def(mv).id.as_str(), "attract" | "captivate" | "taunt") {
        add!(b, "-immune", t, "[from] ability: Oblivious");
        return Val::Null;
    }
    Val::Undef
}
fn pickpocket_after(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if s != t && b.mv(mv).flags & mflag::CONTACT != 0 {
        if b.p(t).item != b.d.c.empty || b.p(t).switch_flag.is_set() || b.p(t).force_switch_flag || b.p(s).switch_flag == SwitchFlag::Yes {
            return Val::Undef;
        }
        let Some(item) = b.take_item(s, Some(t)) else { return Val::Undef };
        if !b.set_item(t, item, None, EffRef::NONE) {
            b.pm(s).item = item;
            return Val::Undef;
        }
        let n = b.eff(item).name.clone();
        let ss = b.mon_str(s);
        add!(b, "-enditem", s, n.clone(), "[silent]", "[from] ability: Pickpocket", format!("[of] {ss}"));
        add!(b, "-item", t, n, "[from] ability: Pickpocket", format!("[of] {ss}"));
    }
    Val::Undef
}
fn prankster_priority(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Status {
        b.mvm(mv).prankster_boosted = true;
        return Val::Num(a.relay.num() + 1.0);
    }
    Val::Undef
}
fn pressure_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    add!(b, "-ability", m, "Pressure");
    Val::Undef
}
fn pressure_deduct_pp(_: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a).map(|s| s.s == t.s).unwrap_or(false) {
        return Val::Undef;
    }
    Val::Num(1.0)
}
fn punkrock_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::SOUND != 0 {
            b.chain_modify(5325.0, 4096.0);
        }
    }
    Val::Undef
}
fn punkrock_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::SOUND != 0 {
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
fn raindish_weather(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let wid = b.eid(a.effect);
    if b.effective_weather(t, EffRef::NONE, false) != wid {
        return Val::Undef;
    }
    if wid == "raindance" || wid == "primordialsea" {
        let h = b.p(t).base_maxhp as f64 / 16.0;
        b.heal(h, None, None, EffRef::NONE);
    }
    Val::Undef
}
fn regenerator_switch_out(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 3.0;
    if b.mon_heal(m, h).is_some() {
        add!(b, "-heal", m, Health(m), "[from] ability: Regenerator", "[silent]");
    }
    Val::Undef
}
fn rockhead_damage(b: &mut Battle, a: &HArgs) -> Val {
    if b.eid(a.effect) == "recoil" {
        if let Some(am) = b.active_move {
            if b.move_def(am).id != "struggle" {
                return Val::Null;
            }
        }
    }
    Val::Undef
}
fn roughskin_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, true) {
        let d = b.p(s).base_maxhp as f64 / 8.0;
        b.damage(d, Some(s), Some(t), EffRef::NONE, false);
    }
    Val::Undef
}
fn sharpness_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::SLICING != 0 {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn shadowtag_trap(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = this_holder(a);
    if !b.has_ability(m, "shadowtag") && b.is_adjacent(m, h) {
        b.try_trap(m, true);
    }
    Val::Undef
}
fn shadowtag_maybe_trap(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let source = src(a).or(match a.holder {
        Holder::Mon(h) => Some(h),
        _ => None,
    });
    let Some(source) = source else { return Val::Undef };
    if !b.is_adjacent(m, source) {
        return Val::Undef;
    }
    if !b.has_ability(m, "shadowtag") {
        b.pm(m).maybe_trapped = true;
    }
    Val::Undef
}
fn solarpower_spa(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if matches!(b.effective_weather(m, EffRef::NONE, false), "sunnyday" | "desolateland") {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn solarpower_weather(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let wid = b.eid(a.effect);
    if b.effective_weather(t, EffRef::NONE, false) != wid {
        return Val::Undef;
    }
    if wid == "sunnyday" || wid == "desolateland" {
        let d = b.p(t).base_maxhp as f64 / 8.0;
        b.damage(d, Some(t), Some(t), EffRef::NONE, false);
    }
    Val::Undef
}
fn solidrock_damage(b: &mut Battle, a: &HArgs) -> Val {
    let Some(def) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.hit_data(mv, def).type_mod > 0 {
        b.chain_modify(0.75, 1.0);
    }
    Val::Undef
}
fn soundproof_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && b.mv(mv).flags & mflag::SOUND != 0 {
        add!(b, "-immune", t, "[from] ability: Soundproof");
        return Val::Null;
    }
    Val::Undef
}
fn soundproof_ally_try_hit_side(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::SOUND != 0 {
        let h = this_holder(a);
        add!(b, "-immune", h, "[from] ability: Soundproof");
    }
    Val::Undef
}
fn speedboost_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).active_turns != 0 {
        let bo = Battle::boosts1(B_SPE, 1);
        b.boost(bo, None, None, EffRef::NONE, false, false);
    }
    Val::Undef
}
fn stalwart_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        let t = b.mv(mv).target != MoveTarget::Scripted;
        b.mvm(mv).tracks_target = t;
    }
    Val::Undef
}
fn stamina_hit(b: &mut Battle, _: &HArgs) -> Val {
    let bo = Battle::boosts1(B_DEF, 1);
    b.boost(bo, None, None, EffRef::NONE, false, false);
    Val::Undef
}
fn steelyspirit_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Steel") {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn sturdy_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some(mv) = amv(a) {
        if b.mv(mv).ohko {
            add!(b, "-immune", t, "[from] ability: Sturdy");
            return Val::Null;
        }
    }
    Val::Undef
}
fn sturdy_damage(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let p = b.p(t);
    if p.hp == p.maxhp && a.relay.num() >= p.hp as f64 && b.etype(a.effect) == EffType::Move {
        add!(b, "-ability", t, "Sturdy");
        return Val::Num((b.p(t).hp - 1) as f64);
    }
    Val::Undef
}
fn superluck_crit(_: &mut Battle, a: &HArgs) -> Val {
    Val::Num(a.relay.num() + 1.0)
}
fn symbiosis_after_use_item(b: &mut Battle, a: &HArgs) -> Val {
    let pokemon = tgt(a);
    if b.p(pokemon).switch_flag.is_set() {
        return Val::Undef;
    }
    let source = this_holder(a);
    let Some(my_item) = b.take_item(source, None) else { return Val::Undef };
    let ok = b.single_event(Ev::TakeItem, EffRef::eff(my_item), StateLoc::Item(source), Arg::Mon(pokemon), Arg::Mon(source), EffRef::eff(a.this_eff), Val::Eff(my_item));
    if !ok.truthy() || !b.set_item(pokemon, my_item, None, EffRef::NONE) {
        b.pm(source).item = my_item;
        return Val::Undef;
    }
    let n = b.eff(my_item).name.clone();
    let ps = b.mon_str(pokemon);
    add!(b, "-activate", source, "ability: Symbiosis", n, format!("[of] {ps}"));
    Val::Undef
}
fn synchronize_after_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if s == t || b.eid(a.effect) == "toxicspikes" {
        return Val::Undef;
    }
    let Val::Eff(st) = a.relay else { return Val::Undef };
    if st == b.d.c.slp || st == b.d.c.frz {
        return Val::Undef;
    }
    add!(b, "-activate", t, "ability: Synchronize");
    // sourceEffect {status: status.id, id: 'synchronize'}
    let sid = b.eff(st).id.clone();
    let se = b.cond(&format!("synchronize:{sid}"));
    b.try_set_status(s, st, Some(t), EffRef::eff(se));
    Val::Undef
}
fn technician_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let bp = b.modify(a.relay.num(), b.event.modifier);
    if bp <= 60.0 {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn telepathy_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if t != s && t.s == s.s && b.mv(mv).category != Category::Status {
        add!(b, "-activate", t, "ability: Telepathy");
        return Val::Null;
    }
    Val::Undef
}
fn thermalexchange_hit(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Fire") {
            let bo = Battle::boosts1(B_ATK, 1);
            b.boost(bo, None, None, EffRef::NONE, false, false);
        }
    }
    Val::Undef
}
fn thermalexchange_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.brn {
        add!(b, "-activate", m, "ability: Thermal Exchange");
        b.cure_status(m, false);
    }
    Val::Undef
}
fn thermalexchange_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay != Val::Eff(b.d.c.brn) {
        return Val::Undef;
    }
    if b.move_status_of(a.effect).is_some() {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Thermal Exchange");
    }
    Val::False
}
fn thickfat_mod(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Ice") || is_type(b, mv, "Fire") {
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
fn toughclaws_base_power(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::CONTACT != 0 {
            b.chain_modify(5325.0, 4096.0);
        }
    }
    Val::Undef
}
fn toxicdebris_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    let side = if s.s == t.s { 1 - s.s } else { s.s };
    let ts = b.cond("toxicspikes");
    let layers = b.state(StateLoc::SideCond(side, ts)).map(|st| st.num(K::Layers));
    if b.mv(mv).category == Category::Physical && (layers.is_none() || layers.unwrap() < 2.0) {
        add!(b, "-activate", t, "ability: Toxic Debris");
        b.add_side_condition(side, ts, Some(t), EffRef::NONE);
    }
    Val::Undef
}
fn trace_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.sset(a.state, K::Started, Val::True);
    let mut seek = true;
    if b.adjacent_foes(m).iter().any(|&f| b.ability_id(f) == "noability") {
        seek = false;
    }
    if b.has_item(m, "abilityshield") {
        add!(b, "-block", m, "item: Ability Shield");
        seek = false;
    }
    b.sset(a.state, K::X1, Val::from_bool(seek));
    if seek {
        b.single_event(Ev::Update, EffRef::eff(a.this_eff), a.state, Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
    }
    Val::Undef
}
fn trace_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if !b.sget(a.state, K::X1).truthy() {
        return Val::Undef;
    }
    let targets: Vec<Mon> = b
        .adjacent_foes(m)
        .into_iter()
        .filter(|&f| {
            let ab = b.eff(b.p(f).ability);
            !ab.ability.as_ref().map(|x| x.notrace).unwrap_or(false) && ab.id != "noability"
        })
        .collect();
    if targets.is_empty() {
        return Val::Undef;
    }
    let i = b.prng.sample_index(targets.len());
    let target = targets[i];
    let ab = b.p(target).ability;
    b.set_ability(m, ab, Some(target), EffRef::NONE, false, false);
    Val::Undef
}
fn unaware_modify_boost(b: &mut Battle, a: &HArgs) -> Val {
    let user = this_holder(a);
    let pokemon = tgt(a);
    if user == pokemon {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if Some(user) == b.active_pokemon && Some(pokemon) == b.active_target {
        bo.set(B_DEF, 0);
        bo.set(B_SPD, 0);
        bo.set(B_EVA, 0);
    }
    if Some(pokemon) == b.active_pokemon && Some(user) == b.active_target {
        bo.set(B_ATK, 0);
        bo.set(B_DEF, 0);
        bo.set(B_SPA, 0);
        bo.set(B_ACC, 0);
    }
    Val::Boosts(bo)
}
fn unburden_after_use_item(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if m != this_holder(a) {
        return Val::Undef;
    }
    let c = b.cond("unburden");
    b.add_volatile(m, c, None, EffRef::NONE, None);
    Val::Undef
}
fn unburden_take_item(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let c = b.cond("unburden");
    b.add_volatile(m, c, None, EffRef::NONE, None);
    Val::Undef
}
fn unburden_end(b: &mut Battle, a: &HArgs) -> Val {
    b.remove_volatile_id(tgt(a), "unburden");
    Val::Undef
}
fn unnerve_start(b: &mut Battle, a: &HArgs) -> Val {
    if b.sget(a.state, K::Unnerved).truthy() {
        return Val::Undef;
    }
    let m = tgt(a);
    add!(b, "-ability", m, "Unnerve");
    b.sset(a.state, K::Unnerved, Val::True);
    Val::Undef
}
fn unnerve_end(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Unnerved, Val::False);
    Val::Undef
}
fn unnerve_foe_try_eat(b: &mut Battle, a: &HArgs) -> Val {
    Val::from_bool(!b.sget(a.state, K::Unnerved).truthy())
}
fn voltabsorb_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    absorb_heal(b, a, "Electric", "ability: Volt Absorb")
}
fn waterabsorb_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    absorb_heal(b, a, "Water", "ability: Water Absorb")
}
fn absorb_heal(b: &mut Battle, a: &HArgs, ty: &str, msg: &str) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, ty) {
        let h = b.p(t).base_maxhp as f64 / 4.0;
        if !b.heal(h, None, None, EffRef::NONE).truthy() {
            add!(b, "-immune", t, format!("[from] {msg}"));
        }
        return Val::Null;
    }
    Val::Undef
}
fn weakarmor_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if let Some(mv) = amv(a) {
        if b.mv(mv).category == Category::Physical {
            let mut bo = SparseBoosts::default();
            bo.set(B_DEF, -1);
            bo.set(B_SPE, 2);
            b.boost(bo, Some(t), Some(t), EffRef::NONE, false, false);
        }
    }
    Val::Undef
}
fn fairyaura_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.suppressing_ability(Some(m)) {
        return Val::Undef;
    }
    add!(b, "-ability", m, "Fairy Aura");
    Val::Undef
}
fn fairyaura_base_power(b: &mut Battle, a: &HArgs) -> Val {
    // onAnyBasePower(basePower, source, target, move)
    let source = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if src(a) == Some(source) || b.mv(mv).category == Category::Status || !is_type(b, mv, "Fairy") {
        return Val::Undef;
    }
    // move.auraBooster: the first Fairy Aura holder that sees the move (single Floette in practice)
    let h = this_holder(a);
    let key = b.mv(mv).aura_booster;
    let booster = match key {
        Some(x) if b.has_ability(x, "fairyaura") => x,
        _ => {
            b.mvm(mv).aura_booster = Some(h);
            h
        }
    };
    if booster != h {
        return Val::Undef;
    }
    b.chain_modify(5448.0, 4096.0);
    Val::Undef
}
fn auraguard_damage(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::CONTACT != 0 {
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
fn noguard_invulnerability(b: &mut Battle, a: &HArgs) -> Val {
    let h = this_holder(a);
    if amv(a).is_some() && (src(a) == Some(h) || a.target.mon() == Some(h)) {
        return Val::Num(0.0);
    }
    Val::Undef
}
fn noguard_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    let h = this_holder(a);
    let _ = b;
    if amv(a).is_some() && (src(a) == Some(h) || a.target.mon() == Some(h)) {
        return Val::True;
    }
    a.relay
}
fn sheerforce_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).secondaries.is_some() && !b.move_def(mv).has_sheer_force_boost {
        b.mvm(mv).secondaries = None;
        b.mvm(mv).self_ = None;
        b.mvm(mv).has_sheer_force = true;
    }
    Val::Undef
}
fn sheerforce_base_power(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).has_sheer_force || b.move_def(mv).has_sheer_force_boost {
        b.chain_modify(5325.0, 4096.0);
    }
    Val::Undef
}
fn firemane_mod(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Fire") {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn megasol_weather_damage(b: &mut Battle, a: &HArgs) -> Val {
    // runs Sunny Day's onWeatherModifyDamage with Mega Sol's context
    let att = tgt(a);
    let Some(def) = src(a) else { return a.relay };
    let Some(mv) = amv(a) else { return a.relay };
    if b.move_def(mv).id == "hydrosteam" && b.effective_weather(att, EffRef::NONE, false) == "sunnyday" {
        b.chain_modify(1.5, 1.0);
        return a.relay;
    }
    if b.effective_weather(def, EffRef::NONE, false) != "sunnyday" {
        return a.relay;
    }
    if is_type(b, mv, "Fire") {
        b.chain_modify(1.5, 1.0);
    } else if is_type(b, mv, "Water") {
        b.chain_modify(0.5, 1.0);
    }
    a.relay
}
fn parentalbond_prepare_hit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    let m = b.mv(mv);
    if m.category == Category::Status || m.multihit.is_some() || m.flags & (mflag::bit("noparentalbond") | mflag::CHARGE | mflag::FUTUREMOVE) != 0 || m.spread_hit {
        return Val::Undef;
    }
    b.mvm(mv).multihit = Some((2, 2));
    b.mvm(mv).multihit_type = 1;
    Val::Undef
}

fn illusion_before_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).illusion = None;
    let s = m.s as usize;
    let pos = b.p(m).position as usize;
    let n = b.sides[s].pokemon.len();
    let mut i = n;
    while i > pos + 1 {
        i -= 1;
        let cand = Mon { s: m.s, i: b.sides[s].pokemon[i] };
        if !b.p(cand).fainted {
            b.pm(m).illusion = Some(cand);
            break;
        }
    }
    Val::Undef
}
fn illusion_damaging_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.p(t).illusion.is_some() {
        let il = b.d.ability("illusion");
        b.single_event(Ev::End, EffRef::eff(il), StateLoc::Ability(t), Arg::Mon(t), a.source, a.effect, Val::Undef);
    }
    Val::Undef
}
fn illusion_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).illusion.is_some() && !b.p(m).being_called_back {
        b.pm(m).illusion = None;
        let det = b.updated_details(m);
        add!(b, "replace", m, det);
        add!(b, "-end", m, "Illusion");
    }
    Val::Undef
}
fn illusion_faint(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.pm(m).illusion = None;
    Val::Undef
}
fn stancechange_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let sp = &b.d.species[b.p(m).species as usize];
    if sp.base_species != "Aegislash" || b.p(m).transformed {
        return Val::Undef;
    }
    let id = b.move_def(mv).id.as_str();
    if b.mv(mv).category == Category::Status && id != "kingsshield" {
        return Val::Undef;
    }
    let forme = if id == "kingsshield" { "Aegislash" } else { "Aegislash-Blade" };
    if sp.name != forme {
        let fid = b.d.species_id(forme).unwrap() as u16;
        // the Champions formeChange has no default source: no "[from]"
        b.forme_change(m, fid, EffRef::NONE, false, None);
    }
    Val::Undef
}

// ------------------------------------------------------------------ added for the M-A / M-B team pool
fn supremeoverlord_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    let tf = b.sides[m.s as usize].total_fainted;
    if tf > 0 {
        add!(b, "-activate", m, "ability: Supreme Overlord");
        let fallen = tf.min(5) as f64;
        add!(b, "-start", m, format!("fallen{}", fallen as u32), "[silent]");
        b.sset(a.state, K::Fallen, Val::Num(fallen));
    }
    Val::Undef
}
fn supremeoverlord_bp(b: &mut Battle, a: &HArgs) -> Val {
    let f = b.sget(a.state, K::Fallen).num();
    if f > 0.0 {
        const POW: [f64; 6] = [4096.0, 4506.0, 4915.0, 5325.0, 5734.0, 6144.0];
        b.chain_modify(POW[(f as usize).min(5)], 4096.0);
    }
    Val::Undef
}
fn sniper_damage(b: &mut Battle, a: &HArgs) -> Val {
    // onModifyDamage(damage, source, target, move): a.source = the target
    let Some(t) = src(a) else { return Val::Undef };
    if let Some(mv) = amv(a) {
        if b.hit_data(mv, t).crit {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 2 for the M-A / M-B team pool
fn guts_atk(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status != b.d.c.empty {
        b.chain_modify(1.5, 1.0);
    }
    Val::Undef
}
fn gooey_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, true) {
        let an = b.eff(a.this_eff).name.clone();
        add!(b, "-ability", t, an);
        let bo = Battle::boosts1(B_SPE, -1);
        b.boost(bo, Some(s), Some(t), EffRef::NONE, true, false);
    }
    Val::Undef
}
fn waterbubble_source(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Fire") {
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
fn waterbubble_mod(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Water") {
            b.chain_modify(2.0, 1.0);
        }
    }
    Val::Undef
}
fn cure_on_update(b: &mut Battle, a: &HArgs, st: EffId, msg: &str) -> Val {
    let m = tgt(a);
    if b.p(m).status == st {
        add!(b, "-activate", m, msg.to_string());
        b.cure_status(m, false);
    }
    Val::Undef
}
fn block_status(b: &mut Battle, a: &HArgs, st: EffId, msg: &str) -> Val {
    if a.relay != Val::Eff(st) {
        return Val::Undef;
    }
    if b.move_status_of(a.effect).is_some() {
        let t = tgt(a);
        add!(b, "-immune", t, format!("[from] {msg}"));
    }
    Val::False
}
fn waterbubble_update(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.brn;
    cure_on_update(b, a, st, "ability: Water Bubble")
}
fn waterbubble_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.brn;
    block_status(b, a, st, "ability: Water Bubble")
}
fn dryskin_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    absorb_heal(b, a, "Water", "ability: Dry Skin")
}
fn dryskin_bp(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Fire") {
            b.chain_modify(1.25, 1.0);
        }
    }
    Val::Undef
}
fn dryskin_weather(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let wid = b.eid(a.effect);
    if b.effective_weather(t, EffRef::NONE, false) != wid {
        return Val::Undef;
    }
    let d = b.p(t).base_maxhp as f64 / 8.0;
    if wid == "raindance" || wid == "primordialsea" {
        b.heal(d, None, None, EffRef::NONE);
    } else if wid == "sunnyday" || wid == "desolateland" {
        b.damage(d, Some(t), Some(t), EffRef::NONE, false);
    }
    Val::Undef
}
fn overcoat_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("sandstorm") || a.relay == Val::Key("hail") || a.relay == Val::Key("powder") {
        return Val::False;
    }
    Val::Undef
}
fn overcoat_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::POWDER != 0 && src(a) != Some(t) && !b.has_type(t, "Grass") {
        add!(b, "-immune", t, "[from] ability: Overcoat");
        return Val::Null;
    }
    Val::Undef
}
fn eartheater_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    absorb_heal(b, a, "Ground", "ability: Earth Eater")
}
fn in_sun(b: &mut Battle, m: Mon) -> bool {
    matches!(b.effective_weather(m, EffRef::NONE, false), "sunnyday" | "desolateland")
}
fn leafguard_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if in_sun(b, t) {
        if b.move_status_of(a.effect).is_some() {
            add!(b, "-immune", t, "[from] ability: Leaf Guard");
        }
        return Val::False;
    }
    Val::Undef
}
fn relay_is(b: &Battle, a: &HArgs, id: &str) -> bool {
    matches!(a.relay, Val::Eff(e) if b.eff(e).id == id)
}
fn leafguard_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if relay_is(b, a, "yawn") && in_sun(b, t) {
        add!(b, "-immune", t, "[from] ability: Leaf Guard");
        return Val::Null;
    }
    Val::Undef
}
fn insomnia_update(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.slp;
    cure_on_update(b, a, st, "ability: Insomnia")
}
fn insomnia_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.slp;
    block_status(b, a, st, "ability: Insomnia")
}
fn insomnia_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if relay_is(b, a, "yawn") {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Insomnia");
        return Val::Null;
    }
    Val::Undef
}
// Vital Spirit = Insomnia (Showdown's code is identical apart from the name)
fn vitalspirit_update(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.slp;
    cure_on_update(b, a, st, "ability: Vital Spirit")
}
fn vitalspirit_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.slp;
    block_status(b, a, st, "ability: Vital Spirit")
}
fn vitalspirit_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if relay_is(b, a, "yawn") {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Vital Spirit");
        return Val::Null;
    }
    Val::Undef
}
fn zerotohero_switch_out(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let sp = &b.d.species[b.p(m).species as usize];
    if sp.base_species != "Palafin" || sp.name == "Palafin-Hero" {
        return Val::Undef;
    }
    if let Some(fid) = b.d.species_id("Palafin-Hero") {
        b.forme_change(m, fid as u16, EffRef::eff(a.this_eff), true, None);
        b.pm(m).hero_message_displayed = false;
    }
    Val::Undef
}
fn zerotohero_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let sp = &b.d.species[b.p(m).species as usize];
    if sp.base_species != "Palafin" {
        return Val::Undef;
    }
    if !b.p(m).hero_message_displayed && sp.name == "Palafin-Hero" {
        add!(b, "-activate", m, "ability: Zero to Hero");
        b.pm(m).hero_message_displayed = true;
    }
    Val::Undef
}
fn reckless_bp(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).recoil.is_some() || b.mv(mv).has_crash_damage {
            b.chain_modify(4915.0, 4096.0);
        }
    }
    Val::Undef
}
fn healer_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for al in b.adjacent_allies(m) {
        if b.p(al).status != b.d.c.empty && b.random_chance(1, 2) {
            add!(b, "-activate", m, "ability: Healer");
            b.cure_status(al, false);
        }
    }
    Val::Undef
}
fn owntempo_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_vol(m, "confusion") {
        add!(b, "-activate", m, "ability: Own Tempo");
        b.remove_volatile_id(m, "confusion");
    }
    Val::Undef
}
fn owntempo_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if relay_is(b, a, "confusion") {
        return Val::Null;
    }
    Val::Undef
}
fn aromaveil_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    let Val::Eff(e) = a.relay else { return Val::Undef };
    if ["attract", "disable", "encore", "healblock", "taunt", "torment"].contains(&b.eff(e).id.as_str()) {
        if b.etype(a.effect) == EffType::Move {
            let t = tgt(a);
            let h = this_holder(a);
            let hs = b.mon_str(h);
            add!(b, "-block", t, "ability: Aroma Veil", format!("[of] {hs}"));
        }
        return Val::Null;
    }
    Val::Undef
}
fn analytic_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for t in b.all_active() {
        if t == m {
            continue;
        }
        if b.will_move(t).is_some() {
            return Val::Undef;
        }
    }
    b.chain_modify(5325.0, 4096.0);
    Val::Undef
}
fn cheekpouch_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 3.0;
    b.heal(h, None, None, EffRef::NONE);
    Val::Undef
}
fn sapsipper_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, "Grass") {
        let bo = Battle::boosts1(B_ATK, 1);
        if !b.boost(bo, None, None, EffRef::NONE, false, false).truthy() {
            add!(b, "-immune", t, "[from] ability: Sap Sipper");
        }
        return Val::Null;
    }
    Val::Undef
}
fn effectspore_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) && b.run_status_immunity(s, "powder", false) {
        let r = b.random(100);
        let st = if r < 11 { "slp" } else if r < 21 { "par" } else if r < 30 { "psn" } else { "" };
        if !st.is_empty() {
            let c = b.cond(st);
            b.try_set_status(s, c, Some(t), EffRef::NONE);
        }
    }
    Val::Undef
}
fn screencleaner_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let mut activated = false;
    for c in ["reflect", "lightscreen", "auroraveil"] {
        for side in [m.s, 1 - m.s] {
            if b.has_side_condition(side, c) {
                if !activated {
                    add!(b, "-activate", m, "ability: Screen Cleaner");
                    activated = true;
                }
                let id = b.cond(c);
                b.remove_side_condition(side, id);
            }
        }
    }
    Val::Undef
}
fn tangledfeet_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    let t = tgt(a);
    if b.has_vol(t, "confusion") {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn heatproof_mod(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if is_type(b, mv, "Fire") {
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
fn heatproof_damage(b: &mut Battle, a: &HArgs) -> Val {
    if !a.effect.is_none() && b.eid(a.effect) == "brn" {
        return Val::Num(a.relay.num() / 2.0);
    }
    Val::Undef
}
fn sweetveil_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Eff(b.d.c.slp) {
        let t = tgt(a);
        let h = this_holder(a);
        let hs = b.mon_str(h);
        add!(b, "-block", t, "ability: Sweet Veil", format!("[of] {hs}"));
        return Val::Null;
    }
    Val::Undef
}
fn sweetveil_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if relay_is(b, a, "yawn") {
        let t = tgt(a);
        let h = this_holder(a);
        let hs = b.mon_str(h);
        add!(b, "-block", t, "ability: Sweet Veil", format!("[of] {hs}"));
        return Val::Null;
    }
    Val::Undef
}
fn curiousmedicine_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for al in b.adjacent_allies(m) {
        b.pm(al).boosts = [0; 7];
        let ms = b.mon_str(m);
        add!(b, "-clearboost", al, "[from] ability: Curious Medicine", format!("[of] {ms}"));
    }
    Val::Undef
}
fn plus_spa(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    for al in b.allies(m) {
        if b.has_ability_any(al, &["minus", "plus"]) {
            b.chain_modify(1.5, 1.0);
            return Val::Undef;
        }
    }
    Val::Undef
}
fn strongjaw_bp(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).flags & mflag::BITE != 0 {
            b.chain_modify(1.5, 1.0);
        }
    }
    Val::Undef
}
fn eelevate_after_faint(b: &mut Battle, a: &HArgs) -> Val {
    if b.etype(a.effect) == EffType::Move {
        let Some(s) = src(a) else { return Val::Undef };
        let best = b.get_best_stat(s, true, true);
        let bo = Battle::boosts1((best - 1) as u8, a.relay.num() as i8);
        b.boost(bo, Some(s), None, EffRef::NONE, false, false);
    }
    Val::Undef
}
fn wanderingspirit_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) {
        b.skill_swap(s, t);
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 3 (Mega abilities)
fn skilllink_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if let Some((lo, hi)) = b.mv(mv).multihit {
        if lo != hi {
            b.mvm(mv).multihit = Some((hi, hi));
        }
    }
    b.mvm(mv).multiaccuracy = false;
    Val::Undef
}
fn unseenfist_hit_protect(b: &mut Battle, a: &HArgs) -> Val {
    // onHitProtect(source, target, move): the event runs on the attacker, a.source = the protected target
    let Some(t) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).flags & mflag::CONTACT != 0 {
        b.hit_data_mut(mv, t).bypass_protect = Some(a.this_eff);
        return Val::False;
    }
    Val::Undef
}
fn innardsout_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.p(t).hp == 0 {
        let mut d = a.relay.num();
        if b.mv(mv).smart_target != Some(true) {
            d += b.mv(mv).total_damage;
        }
        b.damage(d, Some(s), Some(t), EffRef::NONE, false);
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 4 (rare abilities)
fn noop_ab(_: &mut Battle, _: &HArgs) -> Val {
    Val::Undef
}
fn berserk_damage(b: &mut Battle, a: &HArgs) -> Val {
    let multihit = a.effect.is_move() && b.mv(a.effect.mv).multihit.is_some();
    let checked = !(b.etype(a.effect) == EffType::Move && !multihit);
    b.sset(a.state, K::Checked, Val::from_bool(checked));
    Val::Undef
}
fn berserk_try_eat(b: &mut Battle, a: &HArgs) -> Val {
    let Val::Eff(item) = a.relay else { return Val::True };
    let id = b.eff(item).id.as_str();
    if matches!(id, "aguavberry" | "enigmaberry" | "figyberry" | "iapapaberry" | "magoberry" | "sitrusberry" | "wikiberry" | "oranberry" | "berryjuice") {
        return b.sget(a.state, K::Checked);
    }
    Val::True
}
fn berserk_after(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Checked, Val::True);
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if s == t || b.p(t).hp <= 0 || b.mv(mv).total_damage == 0.0 {
        return Val::Undef;
    }
    let Some(last) = b.get_last_attacked_by(t).map(|x| x.damage) else { return Val::Undef };
    let m = b.mv(mv);
    let dmg = if m.multihit.is_some() && m.smart_target != Some(true) { m.total_damage } else { last as f64 };
    let (hp, max) = (b.p(t).hp as f64, b.p(t).maxhp as f64);
    if hp <= max / 2.0 && hp + dmg > max / 2.0 {
        let bo = Battle::boosts1(B_SPA, 1);
        b.boost(bo, Some(t), Some(t), EffRef::NONE, false, false);
    }
    Val::Undef
}
fn magician_after(b: &mut Battle, a: &HArgs) -> Val {
    // onAfterMoveSecondarySelf(source, target, move): a.target = the user
    let s = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.p(s).switch_flag == SwitchFlag::Yes || b.p(s).item != b.d.c.empty || b.has_vol(s, "gem")
        || b.move_def(mv).id == "fling" || b.mv(mv).category == Category::Status
    {
        return Val::Undef;
    }
    let mut targets = b.mv(mv).hit_targets.clone();
    if targets.is_empty() {
        return Val::Undef;
    }
    // speedSort: faster first, ties at random
    if targets.len() == 2 {
        let (x, y) = (b.p(targets[0]).speed, b.p(targets[1]).speed);
        if y > x || (y == x && b.random(2) == 1) {
            targets.swap(0, 1);
        }
    }
    for t in targets {
        if t == s {
            continue;
        }
        let Some(item) = b.take_item(t, Some(s)) else { continue };
        if !b.set_item(s, item, None, EffRef::NONE) {
            b.pm(t).item = item;
            continue;
        }
        let n = b.eff(item).name.clone();
        let ts = b.mon_str(t);
        add!(b, "-item", s, n, "[from] ability: Magician", format!("[of] {ts}"));
        return Val::Undef;
    }
    Val::Undef
}
fn imposter_switch_in(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let pos = b.p(m).position as usize;
    if let Some(t) = b.active_at(1 - m.s as usize, 1 - pos) {
        let e = EffRef::eff(a.this_eff);
        b.transform_into(m, t, e);
    }
    Val::Undef
}
fn mummy_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    let sa = b.p(s).ability;
    let cant = b.eff(sa).ability.as_ref().map(|x| x.cantsuppress).unwrap_or(false);
    if cant || b.eff(sa).id == "mummy" {
        return Val::Undef;
    }
    let announce = !b.is_ally(s, Some(t));
    if b.check_move_makes_contact(mv, s, t, announce) {
        let mummy = b.d.ability("mummy");
        b.set_ability(s, mummy, Some(t), EffRef::NONE, false, false);
    }
    Val::Undef
}
fn hungerswitch_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let sp = &b.d.species[b.p(m).species as usize];
    if sp.base_species != "Morpeko" {
        return Val::Undef;
    }
    let target = if sp.name == "Morpeko" { "Morpeko-Hangry" } else { "Morpeko" };
    if let Some(fid) = b.d.species_id(target) {
        b.forme_change(m, fid as u16, EffRef::NONE, false, None);
    }
    Val::Undef
}
fn keeneye_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a) == Some(t) {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if bo.get(B_ACC).map(|v| v < 0).unwrap_or(false) {
        bo.remove(B_ACC);
        if !effect_has_secondaries(b, a.effect) {
            let an = b.eff(a.this_eff).name.clone();
            let ts = b.mon_str(t);
            add!(b, "-fail", t, "unboost", "accuracy", format!("[from] ability: {an}"), format!("[of] {ts}"));
        }
        return Val::Boosts(bo);
    }
    Val::Undef
}
fn keeneye_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    if let Some(mv) = amv(a) {
        b.mvm(mv).ignore_evasion = true;
    }
    Val::Undef
}
fn bigpecks_try_boost(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if src(a) == Some(t) {
        return Val::Undef;
    }
    let Val::Boosts(mut bo) = a.relay else { return Val::Undef };
    if bo.get(B_DEF).map(|v| v < 0).unwrap_or(false) {
        bo.remove(B_DEF);
        if !effect_has_secondaries(b, a.effect) && b.eid(a.effect) != "octolock" {
            let ts = b.mon_str(t);
            add!(b, "-fail", t, "unboost", "def", "[from] ability: Big Pecks", format!("[of] {ts}"));
        }
        return Val::Boosts(bo);
    }
    Val::Undef
}
fn supersweetsyrup_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).syrup_triggered {
        return Val::Undef;
    }
    b.pm(m).syrup_triggered = true;
    add!(b, "-ability", m, "Supersweet Syrup");
    for t in b.adjacent_foes(m) {
        if b.has_vol(t, "substitute") {
            add!(b, "-immune", t);
        } else {
            let bo = Battle::boosts1(B_EVA, -1);
            b.boost(bo, Some(t), Some(m), EffRef::NONE, true, false);
        }
    }
    Val::Undef
}

fn supremeoverlord_end(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    let f = b.sget(a.state, K::Fallen).num();
    if b.p(m).being_called_back || f <= 0.0 {
        return Val::Undef;
    }
    add!(b, "-end", m, format!("fallen{}", f as u32), "[silent]");
    Val::Undef
}
fn quickdraw_priority(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category != Category::Status && b.random_chance(3, 10) {
        add!(b, "-activate", m, "ability: Quick Draw");
        return Val::Num(0.1);
    }
    Val::Undef
}
fn magmaarmor_update(b: &mut Battle, a: &HArgs) -> Val {
    let st = b.d.c.frz;
    cure_on_update(b, a, st, "ability: Magma Armor")
}
fn magmaarmor_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("frz") {
        return Val::False;
    }
    Val::Undef
}
fn hustle_atk(b: &mut Battle, a: &HArgs) -> Val {
    Val::Num(b.modify(a.relay.num(), 1.5))
}
fn hustle_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Physical && a.relay.is_num() {
        b.chain_modify(3277.0, 4096.0);
    }
    Val::Undef
}
fn immunity_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let st = b.p(m).status;
    if st == b.d.c.psn || st == b.d.c.tox {
        add!(b, "-activate", m, "ability: Immunity");
        b.cure_status(m, false);
    }
    Val::Undef
}
fn immunity_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay != Val::Eff(b.d.c.psn) && a.relay != Val::Eff(b.d.c.tox) {
        return Val::Undef;
    }
    if b.move_status_of(a.effect).is_some() {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Immunity");
    }
    Val::False
}
fn shielddust_secondaries(b: &mut Battle, a: &HArgs) -> Val {
    // only the secondaries with a `self` part survive
    let Some(mv) = amv(a) else { return Val::Undef };
    let Val::Num(x) = a.relay else { return Val::Undef };
    let mut mask = x as u32;
    if let Some(secs) = b.mv(mv).secondaries.as_ref() {
        for (k, sec) in secs.iter().enumerate() {
            if sec.self_.is_none() {
                mask &= !(1u32 << k);
            }
        }
    }
    Val::Num(mask as f64)
}
fn cutecharm_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) && b.random_chance(3, 10) {
        let at = b.cond("attract");
        b.add_volatile(s, at, Some(t), EffRef::NONE, None);
    }
    Val::Undef
}
fn rivalry_bp(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    let Some(d) = src(a) else { return Val::Undef };
    let (ga, gd) = (b.p(m).gender, b.p(d).gender);
    if !ga.is_empty() && !gd.is_empty() {
        if ga == gd {
            b.chain_modify(1.25, 1.0);
        } else {
            b.chain_modify(0.75, 1.0);
        }
    }
    Val::Undef
}

fn merciless_crit(b: &mut Battle, a: &HArgs) -> Val {
    let Some(t) = src(a) else { return Val::Undef };
    let st = b.p(t).status;
    if st == b.d.c.psn || st == b.d.c.tox {
        return Val::Num(5.0);
    }
    Val::Undef
}

fn motordrive_try_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(t) != src(a) && is_type(b, mv, "Electric") {
        let bo = Battle::boosts1(B_SPE, 1);
        if !b.boost(bo, None, None, EffRef::NONE, false, false).truthy() {
            add!(b, "-immune", t, "[from] ability: Motor Drive");
        }
        return Val::Null;
    }
    Val::Undef
}
fn purifyingsalt_set_status(b: &mut Battle, a: &HArgs) -> Val {
    if b.move_status_of(a.effect).is_some() {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Purifying Salt");
    }
    Val::False
}
fn purifyingsalt_try_add_volatile(b: &mut Battle, a: &HArgs) -> Val {
    if relay_is(b, a, "yawn") {
        let t = tgt(a);
        add!(b, "-immune", t, "[from] ability: Purifying Salt");
        return Val::Null;
    }
    Val::Undef
}
fn purifyingsalt_weaken(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if is_type(b, mv, "Ghost") {
        b.chain_modify(0.5, 1.0);
    }
    Val::Undef
}
fn harvest_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.is_weather(&["sunnyday", "desolateland"]) || b.random_chance(1, 2) {
        let last = b.p(m).last_item;
        let berry = last != b.d.c.empty && b.eff(last).item.as_ref().map(|i| i.is_berry).unwrap_or(false);
        if b.p(m).hp > 0 && b.p(m).item == b.d.c.empty && berry {
            b.set_item(m, last, None, EffRef::NONE);
            b.pm(m).last_item = b.d.c.empty;
            let n = b.eff(last).name.clone();
            add!(b, "-item", m, n, "[from] ability: Harvest");
        }
    }
    Val::Undef
}
fn opportunist_foe_after_boost(b: &mut Battle, a: &HArgs) -> Val {
    // onFoeAfterBoost(boost, target, source, effect): collect the foe's stat rises (not Opportunist's / Mirror Herb's own)
    if !a.effect.is_none() && matches!(b.eid(a.effect), "opportunist" | "mirrorherb") {
        return Val::Undef;
    }
    let Val::Boosts(bo) = a.relay else { return Val::Undef };
    let mut acc = match b.sget(a.state, K::Boosts) {
        Val::Boosts(x) => x,
        _ => SparseBoosts::default(),
    };
    for (k, v) in bo.iter() {
        if v > 0 {
            acc.set(k, acc.get(k).unwrap_or(0) + v);
        }
    }
    // (Showdown creates the table on any foe boost, even an empty one: the flush then boosts nothing)
    b.sset(a.state, K::Boosts, Val::Boosts(acc));
    Val::Undef
}
fn opportunist_flush(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    if let Val::Boosts(bo) = b.sget(a.state, K::Boosts) {
        b.sset(a.state, K::Boosts, Val::Undef);
        b.boost(bo, Some(m), None, EffRef::eff(a.this_eff), false, false);
    }
    Val::Undef
}
fn opportunist_end(b: &mut Battle, a: &HArgs) -> Val {
    b.sset(a.state, K::Boosts, Val::Undef);
    Val::Undef
}
fn cudchew_eat_item(b: &mut Battle, a: &HArgs) -> Val {
    // onEatItem(item, pokemon, source, effect)
    let Val::Eff(item) = a.relay else { return Val::Undef };
    let berry = b.eff(item).item.as_ref().map(|i| i.is_berry).unwrap_or(false);
    let by_bite = !a.effect.is_none() && matches!(b.eid(a.effect), "bugbite" | "pluck");
    if berry && !by_bite {
        b.sset(a.state, K::X1, Val::Eff(item));
        // counter = 2, minus 1 when nothing else is queued this turn
        let c = if b.queue.is_empty() { 1.0 } else { 2.0 };
        b.sset(a.state, K::Counter, Val::Num(c));
    }
    Val::Undef
}
fn cudchew_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let Val::Eff(item) = b.sget(a.state, K::X1) else { return Val::Undef };
    if b.p(m).hp == 0 {
        return Val::Undef;
    }
    let c = b.sget(a.state, K::Counter).num() - 1.0;
    b.sset(a.state, K::Counter, Val::Num(c));
    if c <= 0.0 {
        add!(b, "-activate", m, "ability: Cud Chew");
        let n = b.eff(item).name.clone();
        add!(b, "-enditem", m, n, "[eat]");
        if b.single_event(Ev::Eat, EffRef::eff(item), StateLoc::None, Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef).truthy() {
            b.run_event(Ev::EatItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item));
        }
        if b.eff(item).has(b.d.cbs(Ev::Eat)[P_ON]) {
            b.pm(m).ate_berry = true;
        }
        b.sset(a.state, K::X1, Val::Undef);
        b.sset(a.state, K::Counter, Val::Undef);
    }
    Val::Undef
}
fn sapsipper_ally_try_hit_side(b: &mut Battle, a: &HArgs) -> Val {
    // onAllyTryHitSide(target, source, move)
    let h = holder(a);
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    if s == h || !b.is_ally(t, Some(s)) {
        return Val::Undef;
    }
    let Some(mv) = amv(a) else { return Val::Undef };
    if is_type(b, mv, "Grass") {
        let bo = Battle::boosts1(B_ATK, 1);
        b.boost(bo, Some(h), None, EffRef::NONE, false, false);
    }
    Val::Undef
}

fn mimicry_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.single_event(Ev::TerrainChange, EffRef::eff(a.this_eff), a.state, Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
    Val::Undef
}
fn mimicry_terrain_change(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let terrain = b.eff(b.field.terrain).id.clone();
    let types: Vec<TypeId> = match terrain.as_str() {
        "electricterrain" => vec![b.d.type_id("Electric")],
        "grassyterrain" => vec![b.d.type_id("Grass")],
        "mistyterrain" => vec![b.d.type_id("Fairy")],
        "psychicterrain" => vec![b.d.type_id("Psychic")],
        _ => b.d.species[b.p(m).base_species as usize].types.clone(),
    };
    if b.get_types(m, false) == types || !b.set_type(m, types.clone(), false) {
        return Val::Undef;
    }
    if !terrain.is_empty() || b.p(m).transformed {
        let shown: Vec<String> = types.iter().map(|&t| b.d.type_name(t).to_string()).collect();
        add!(b, "-start", m, "typechange", shown.join("/"), "[from] ability: Mimicry");
    } else {
        add!(b, "-activate", m, "ability: Mimicry");
        add!(b, "-end", m, "typechange", "[silent]");
    }
    Val::Undef
}

fn icebody_weather(b: &mut Battle, a: &HArgs) -> Val {
    // onWeather(target, source, effect)
    let t = tgt(a);
    if !a.effect.is_none() && matches!(b.eid(a.effect), "hail" | "snowscape") {
        let h = b.p(t).base_maxhp as f64 / 16.0;
        b.heal(h, Some(t), None, EffRef::NONE);
    }
    Val::Undef
}
fn icebody_immunity(_: &mut Battle, a: &HArgs) -> Val {
    if a.relay == Val::Key("hail") {
        return Val::False;
    }
    Val::Undef
}
fn forecast_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.single_event(Ev::WeatherChange, EffRef::eff(a.this_eff), a.state, Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
    Val::Undef
}
fn forecast_weather_change(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.d.species[b.p(m).base_species as usize].base_species != "Castform" || b.p(m).transformed {
        return Val::Undef;
    }
    let cur = b.d.species[b.p(m).species as usize].name.clone();
    let w = b.effective_weather(m, EffRef::NONE, false).to_string();
    let want = match w.as_str() {
        "sunnyday" | "desolateland" => "Castform-Sunny",
        "raindance" | "primordialsea" => "Castform-Rainy",
        "hail" | "snowscape" => "Castform-Snowy",
        _ => "Castform",
    };
    if b.p(m).is_active && cur != want {
        if let Some(fid) = b.d.species_id(want) {
            b.forme_change(m, fid as u16, EffRef::eff(a.this_eff), false, Some("[msg]"));
        }
    }
    Val::Undef
}

fn heavymetal_weight(_: &mut Battle, a: &HArgs) -> Val {
    Val::Num(a.relay.num() * 2.0)
}
fn lightmetal_weight(_: &mut Battle, a: &HArgs) -> Val {
    Val::Num((a.relay.num() / 2.0).trunc().max(1.0))
}
