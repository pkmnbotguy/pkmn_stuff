//! Item handlers (data/items.ts with the Champions mod) for the items in the team pool.

#![allow(unused_imports)]
use super::{amv, holder, src, tgt};
use crate::add;
use crate::sim::battle::*;
use crate::sim::data::*;
use crate::sim::events::Ev;
use crate::sim::state::*;
use crate::sim::val::{Mon, Val};

type H = fn(&mut Battle, &HArgs) -> Val;

/// type-boosting items: id -> type
const BOOSTERS: &[(&str, &str)] = &[
    ("blackglasses", "Dark"),
    ("charcoal", "Fire"),
    ("fairyfeather", "Fairy"),
    ("hardstone", "Rock"),
    ("magnet", "Electric"),
    ("miracleseed", "Grass"),
    ("mysticwater", "Water"),
    ("nevermeltice", "Ice"),
    ("sharpbeak", "Flying"),
    ("softsand", "Ground"),
    ("twistedspoon", "Psychic"),
    ("spelltag", "Ghost"),
    ("silkscarf", "Normal"),
    ("blackbelt", "Fighting"),
    ("metalcoat", "Steel"),
    ("dragonfang", "Dragon"),
    ("poisonbarb", "Poison"),
    ("silverpowder", "Bug"),
];
/// resist berries: id -> type
const RESIST: &[(&str, &str)] = &[
    ("babiriberry", "Steel"),
    ("chopleberry", "Fighting"),
    ("cobaberry", "Flying"),
    ("colburberry", "Dark"),
    ("habanberry", "Dragon"),
    ("kasibberry", "Ghost"),
    ("occaberry", "Fire"),
    ("passhoberry", "Water"),
    ("rindoberry", "Grass"),
    ("roseliberry", "Fairy"),
    ("tangaberry", "Bug"),
    ("shucaberry", "Ground"),
    ("wacanberry", "Electric"),
    ("yacheberry", "Ice"),
    ("payapaberry", "Psychic"),
    ("kebiaberry", "Poison"),
    ("chartiberry", "Rock"),
];

pub fn lookup(id: &str, cb: &str) -> Option<HandlerFn> {
    if cb == "onBasePower" && BOOSTERS.iter().any(|x| x.0 == id) {
        return Some(type_booster);
    }
    if id == "chilanberry" {
        return match cb {
            "onSourceModifyDamage" => Some(chilan_berry),
            "onEat" => Some(noop),
            _ => None,
        };
    }
    if RESIST.iter().any(|x| x.0 == id) {
        return match cb {
            "onSourceModifyDamage" => Some(resist_berry),
            "onEat" => Some(noop),
            _ => None,
        };
    }
    let f: H = match (id, cb) {
        ("kingsrock", "onModifyMove") => kingsrock_modify_move,
        ("redcard", "onAfterMoveSecondary") => redcard_after,

        ("scopelens", "onModifyCritRatio") => scopelens_crit,
        ("bigroot", "onTryHeal") => bigroot_heal,
        ("focusband", "onDamage") => focusband_damage,

        ("quickclaw", "onFractionalPriority") => quickclaw_priority,

        (_, "onTakeItem") if id.ends_with("ite") || id.ends_with("itex") || id.ends_with("itey") || id.ends_with("itez") => mega_stone_take,
        ("brightpowder", "onModifyAccuracy") => brightpowder_accuracy,
        ("chestoberry", "onUpdate") => chesto_update,
        ("chestoberry", "onEat") => chesto_eat,
        ("choicescarf", "onStart") => choice_start,
        ("choicescarf", "onModifyMove") => choice_modify_move,
        ("choicescarf", "onModifySpe") => choicescarf_spe,
        ("ejectbutton", "onAfterMoveSecondary") => ejectbutton_after,
        ("electricseed", "onStart") => seed_start,
        ("electricseed", "onTerrainChange") => seed_terrain_change,
        ("grassyseed", "onStart") => seed_start,
        ("grassyseed", "onTerrainChange") => seed_terrain_change,
        ("psychicseed", "onStart") => seed_start,
        ("psychicseed", "onTerrainChange") => seed_terrain_change,
        ("mistyseed", "onStart") => seed_start,
        ("mistyseed", "onTerrainChange") => seed_terrain_change,
        ("expertbelt", "onModifyDamage") => expertbelt_damage,
        ("muscleband", "onBasePower") => muscleband_bp,
        ("shellbell", "onAfterMoveSecondarySelf") => shellbell_after,
        ("wiseglasses", "onBasePower") => wiseglasses_bp,
        ("airballoon", "onStart") => airballoon_start,
        ("airballoon", "onDamagingHit") => airballoon_hit,
        ("airballoon", "onAfterSubDamage") => airballoon_after_sub,
        ("focussash", "onDamage") => focussash_damage,
        ("ironball", "onEffectiveness") => ironball_effectiveness,
        ("ironball", "onModifySpe") => half,
        ("leek", "onModifyCritRatio") => leek_crit,
        ("leftovers", "onResidual") => leftovers_residual,
        ("leppaberry", "onUpdate") => leppa_update,
        ("leppaberry", "onEat") => leppa_eat,
        ("lifeorb", "onModifyDamage") => lifeorb_damage,
        ("lifeorb", "onAfterMoveSecondarySelf") => lifeorb_after,
        ("lightball", "onModifyAtk") => lightball_mod,
        ("lightball", "onModifySpA") => lightball_mod,
        ("lumberry", "onAfterSetStatus") => lum_after_set_status,
        ("lumberry", "onUpdate") => lum_update,
        ("lumberry", "onEat") => lum_eat,
        ("mentalherb", "onUpdate") => mentalherb_update,
        ("normalgem", "onSourceTryPrimaryHit") => normalgem_try_primary_hit,
        ("rockyhelmet", "onDamagingHit") => rockyhelmet_hit,
        ("sitrusberry", "onUpdate") => sitrus_update,
        ("sitrusberry", "onTryEatItem") => sitrus_try_eat,
        ("sitrusberry", "onEat") => sitrus_eat,
        ("whiteherb", "onStart") => whiteherb_start,
        ("whiteherb", "onAnySwitchIn") => whiteherb_any,
        ("whiteherb", "onAnyAfterMega") => whiteherb_any,
        ("whiteherb", "onAnyAfterMove") => whiteherb_any,
        ("whiteherb", "onResidual") => whiteherb_start,
        ("whiteherb", "onUse") => whiteherb_use,
        ("widelens", "onSourceModifyAccuracy") => widelens_accuracy,
        ("zoomlens", "onSourceModifyAccuracy") => zoomlens_accuracy,
        _ => return None,
    };
    Some(f)
}

fn noop(_: &mut Battle, _: &HArgs) -> Val {
    Val::Undef
}
fn half(b: &mut Battle, _: &HArgs) -> Val {
    b.chain_modify(0.5, 1.0);
    Val::Undef
}
fn type_booster(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    let id = b.eff(a.this_eff).id.as_str();
    let t = BOOSTERS.iter().find(|x| x.0 == id).map(|x| x.1).unwrap_or("");
    if b.mv(mv).type_ == b.d.type_id(t) {
        b.chain_modify(4915.0, 4096.0);
    }
    Val::Undef
}
/// onSourceModifyDamage(damage, source, target, move): target = the holder (event source)
fn resist_berry(b: &mut Battle, a: &HArgs) -> Val {
    let Some(target) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    let id = b.eff(a.this_eff).id.as_str();
    let t = RESIST.iter().find(|x| x.0 == id).map(|x| x.1).unwrap_or("");
    if b.mv(mv).type_ == b.d.type_id(t) && b.hit_data(mv, target).type_mod > 0 {
        let hit_sub = b.has_volatile(target, b.d.c.substitute) && b.mv(mv).flags & mflag::BYPASSSUB == 0 && !b.mv(mv).infiltrates;
        if hit_sub {
            return Val::Undef;
        }
        if b.eat_item(target, false, None, EffRef::NONE) {
            let n = b.eff(a.this_eff).name.clone();
            add!(b, "-enditem", target, n, "[weaken]");
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
/// Chilan Berry: halves any Normal-type hit (no super-effective condition, unlike the resist berries)
fn chilan_berry(b: &mut Battle, a: &HArgs) -> Val {
    let Some(target) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).type_ == b.d.type_id("Normal") {
        let hit_sub = b.has_volatile(target, b.d.c.substitute) && b.mv(mv).flags & mflag::BYPASSSUB == 0 && !b.mv(mv).infiltrates;
        if hit_sub {
            return Val::Undef;
        }
        if b.eat_item(target, false, None, EffRef::NONE) {
            let n = b.eff(a.this_eff).name.clone();
            add!(b, "-enditem", target, n, "[weaken]");
            b.chain_modify(0.5, 1.0);
        }
    }
    Val::Undef
}
/// onTakeItem(item, source): source = the holder
fn mega_stone_take(b: &mut Battle, a: &HArgs) -> Val {
    let holder = tgt(a);
    let Val::Eff(item) = a.relay else { return Val::True };
    let Some(info) = b.eff(item).item.as_ref() else { return Val::True };
    if info.mega_stone.is_empty() {
        return Val::True;
    }
    let bs = &b.d.species[b.p(holder).base_species as usize];
    let iid = b.eff(item).id.as_str();
    if iid == "floettite" || iid == "meowsticite" {
        let blocked = info.mega_stone.iter().any(|(from, to)| *from == bs.name || *to == bs.name);
        return Val::from_bool(!blocked);
    }
    let blocked = info.mega_stone.iter().any(|(from, _)| *from == bs.base_species);
    Val::from_bool(!blocked)
}
fn brightpowder_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if !a.relay.is_num() {
        return Val::Undef;
    }
    b.chain_modify(3686.0, 4096.0);
    Val::Undef
}
fn chesto_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.slp {
        b.eat_item(m, false, None, EffRef::NONE);
    }
    Val::Undef
}
fn chesto_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status == b.d.c.slp {
        b.cure_status(m, false);
    }
    Val::Undef
}
fn choice_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let c = b.d.c.choicelock;
    if b.has_volatile(m, c) {
        b.remove_volatile(m, c);
    }
    Val::Undef
}
fn choice_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let c = b.d.c.choicelock;
    b.add_volatile(m, c, None, EffRef::NONE, None);
    Val::Undef
}
fn choicescarf_spe(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.has_vol(m, "dynamax") {
        return Val::Undef;
    }
    b.chain_modify(1.5, 1.0);
    Val::Undef
}
fn ejectbutton_after(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if s != t && b.p(t).hp > 0 && b.mv(mv).category != Category::Status && b.mv(mv).flags & mflag::FUTUREMOVE == 0 {
        if b.can_switch(t.s as usize) == 0 || b.p(t).force_switch_flag || b.p(t).being_called_back || b.is_sky_dropped(t) {
            return Val::Undef;
        }
        if b.has_vol(t, "commanding") || b.has_vol(t, "commanded") {
            return Val::Undef;
        }
        for m in b.all_active() {
            if b.p(m).switch_flag == SwitchFlag::Yes {
                return Val::Undef;
            }
        }
        b.pm(t).switch_flag = SwitchFlag::Yes;
        if !b.use_item(t, None, EffRef::NONE) {
            b.pm(t).switch_flag = SwitchFlag::No;
        }
    }
    Val::Undef
}
fn seed_terrain(id: &str) -> &'static str {
    match id {
        "electricseed" => "electricterrain",
        "grassyseed" => "grassyterrain",
        "psychicseed" => "psychicterrain",
        _ => "mistyterrain",
    }
}
fn seed_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let t = seed_terrain(&b.eff(a.this_eff).id);
    if !b.ignoring_item(m) && b.is_terrain(t, None) {
        b.use_item(m, None, EffRef::NONE);
    }
    Val::Undef
}
fn seed_terrain_change(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let t = seed_terrain(&b.eff(a.this_eff).id);
    if b.is_terrain(t, None) {
        b.use_item(m, None, EffRef::NONE);
    }
    Val::Undef
}
fn expertbelt_damage(b: &mut Battle, a: &HArgs) -> Val {
    let Some(def) = src(a) else { return Val::Undef };
    if let Some(mv) = amv(a) {
        if b.hit_data(mv, def).type_mod > 0 {
            b.chain_modify(4915.0, 4096.0);
        }
    }
    Val::Undef
}
fn focussash_damage(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let p = b.p(t);
    if p.hp == p.maxhp && a.relay.num() >= p.hp as f64 && b.etype(a.effect) == EffType::Move {
        if b.use_item(t, None, EffRef::NONE) {
            return Val::Num((b.p(t).hp - 1) as f64);
        }
    }
    Val::Undef
}
fn ironball_effectiveness(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.has_vol(t, "ingrain") || b.has_vol(t, "smackdown") || b.has_pseudo("gravity") {
        return Val::Undef;
    }
    if let Some(mv) = amv(a) {
        if b.mv(mv).type_ == b.d.type_id("Ground") && b.has_type(t, "Flying") {
            return Val::Num(0.0);
        }
    }
    Val::Undef
}
fn leek_crit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let bs = to_id(&b.d.species[b.p(m).base_species as usize].base_species);
    if bs == "farfetchd" || bs == "sirfetchd" {
        return Val::Num(a.relay.num() + 2.0);
    }
    Val::Undef
}
fn leftovers_residual(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 16.0;
    b.heal(h, None, None, EffRef::NONE);
    Val::Undef
}
fn leppa_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).hp == 0 {
        return Val::Undef;
    }
    if b.p(m).move_slots.iter().any(|s| s.pp == 0) {
        b.eat_item(m, false, None, EffRef::NONE);
    }
    Val::Undef
}
fn leppa_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let idx = b.p(m).move_slots.iter().position(|s| s.pp == 0).or_else(|| b.p(m).move_slots.iter().position(|s| s.pp < s.maxpp));
    let Some(i) = idx else { return Val::Undef };
    let added = if b.has_ability(m, "ripen") { 20 } else { 10 };
    let slot = &mut b.pm(m).move_slots[i];
    slot.pp = (slot.pp + added).min(slot.maxpp);
    let n = b.d.moves[b.p(m).move_slots[i].id as usize].name.clone();
    add!(b, "-activate", m, "item: Leppa Berry", n, "[consumed]");
    Val::Undef
}
fn lifeorb_damage(b: &mut Battle, _: &HArgs) -> Val {
    b.chain_modify(5324.0, 4096.0);
    Val::Undef
}
fn lifeorb_after(b: &mut Battle, a: &HArgs) -> Val {
    // onAfterMoveSecondarySelf(source, target, move): source = the holder (event target)
    let s = tgt(a);
    let t = src(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    if Some(s) != t && b.mv(mv).category != Category::Status && !b.p(s).force_switch_flag {
        let d = b.p(s).base_maxhp as f64 / 10.0;
        let lo = b.d.item("lifeorb");
        b.damage(d, Some(s), Some(s), EffRef::eff(lo), false);
    }
    Val::Undef
}
fn lightball_mod(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.d.species[b.p(m).base_species as usize].base_species == "Pikachu" {
        b.chain_modify(2.0, 1.0);
    }
    Val::Undef
}
fn lum_after_set_status(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.eat_item(m, false, None, EffRef::NONE);
    Val::Undef
}
fn lum_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).status != b.d.c.empty || b.has_volatile(m, b.d.c.confusion) {
        b.eat_item(m, false, None, EffRef::NONE);
    }
    Val::Undef
}
fn lum_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    b.cure_status(m, false);
    let c = b.d.c.confusion;
    b.remove_volatile(m, c);
    Val::Undef
}
fn mentalherb_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let conds = ["attract", "taunt", "encore", "torment", "disable", "healblock"];
    for first in conds {
        if b.has_vol(m, first) {
            if !b.use_item(m, None, EffRef::NONE) {
                return Val::Undef;
            }
            for second in conds {
                b.remove_volatile_id(m, second);
                if first == "attract" && second == "attract" {
                    add!(b, "-end", m, "move: Attract", "[from] item: Mental Herb");
                }
            }
            return Val::Undef;
        }
    }
    Val::Undef
}
fn normalgem_try_primary_hit(b: &mut Battle, a: &HArgs) -> Val {
    // onSourceTryPrimaryHit(target, source, move): source = the holder
    let target = tgt(a);
    let Some(source) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if target == source || b.mv(mv).category == Category::Status || b.mv(mv).flags & mflag::bit("pledgecombo") != 0 {
        return Val::Undef;
    }
    if b.mv(mv).type_ == b.d.type_id("Normal") && b.use_item(source, None, EffRef::NONE) {
        let g = b.cond("gem");
        b.add_volatile(source, g, None, EffRef::NONE, None);
    }
    Val::Undef
}
fn rockyhelmet_hit(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.check_move_makes_contact(mv, s, t, false) {
        let d = b.p(s).base_maxhp as f64 / 6.0;
        b.damage(d, Some(s), Some(t), EffRef::NONE, false);
    }
    Val::Undef
}
fn sitrus_update(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if b.p(m).hp as f64 <= b.p(m).maxhp as f64 / 2.0 {
        b.eat_item(m, false, None, EffRef::NONE);
    }
    Val::Undef
}
fn sitrus_try_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 4.0;
    if !b.run_event(Ev::TryHeal, Arg::Mon(m), None, EffRef::eff(a.this_eff), Val::Num(h)).truthy() {
        return Val::False;
    }
    Val::Undef
}
fn sitrus_eat(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    let h = b.p(m).base_maxhp as f64 / 4.0;
    b.heal(h, None, None, EffRef::NONE);
    Val::Undef
}
fn whiteherb_check(b: &mut Battle, m: Mon, state: StateLoc) {
    let mut bo = SparseBoosts::default();
    let mut ready = false;
    for k in 0..7u8 {
        if b.p(m).boosts[k as usize] < 0 {
            ready = true;
            bo.set(k, 0);
        }
    }
    b.sset(state, K::Boosts, Val::Boosts(bo));
    if ready {
        b.use_item(m, None, EffRef::NONE);
    }
    if let Some(st) = b.state_mut(state) {
        st.del(K::Boosts);
    }
}
fn whiteherb_start(b: &mut Battle, a: &HArgs) -> Val {
    // onStart(pokemon) uses this.effectState.target for useItem; the holder here
    let m = holder(a);
    whiteherb_check(b, m, a.state);
    Val::Undef
}
fn whiteherb_any(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    whiteherb_check(b, m, a.state);
    Val::Undef
}
fn whiteherb_use(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if let Val::Boosts(bo) = b.sget(a.state, K::Boosts) {
        b.set_boost(m, &bo);
    }
    add!(b, "-clearnegativeboost", m, "[silent]");
    Val::Undef
}
fn widelens_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    if a.relay.is_num() {
        b.chain_modify(4505.0, 4096.0);
    }
    Val::Undef
}
fn zoomlens_accuracy(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if a.relay.is_num() && b.will_move(t).is_none() {
        b.chain_modify(4915.0, 4096.0);
    }
    Val::Undef
}

// ------------------------------------------------------------------ added for the M-A / M-B team pool
fn quickclaw_priority(b: &mut Battle, a: &HArgs) -> Val {
    let m = holder(a);
    if let Some(mv) = amv(a) {
        if b.mv(mv).category == Category::Status && b.has_ability(m, "myceliummight") {
            return Val::Undef;
        }
    }
    if a.relay.num() <= 0.0 && b.random_chance(1, 5) {
        add!(b, "-activate", m, "item: Quick Claw");
        return Val::Num(0.1);
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 2 for the M-A / M-B team pool
fn scopelens_crit(_: &mut Battle, a: &HArgs) -> Val {
    Val::Num(a.relay.num() + 1.0)
}
fn bigroot_heal(b: &mut Battle, a: &HArgs) -> Val {
    if a.effect.is_none() {
        return Val::Undef;
    }
    let id = b.eid(a.effect);
    if ["drain", "leechseed", "ingrain", "aquaring", "strengthsap"].contains(&id) {
        b.chain_modify(5324.0, 4096.0);
    }
    Val::Undef
}
fn focusband_damage(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    if b.random_chance(1, 10) && a.relay.num() >= b.p(t).hp as f64 && b.etype(a.effect) == EffType::Move {
        add!(b, "-activate", t, "item: Focus Band");
        return Val::Num((b.p(t).hp - 1) as f64);
    }
    Val::Undef
}

// ------------------------------------------------------------------ batch 4 (rare items)
fn kingsrock_modify_move(b: &mut Battle, a: &HArgs) -> Val {
    let Some(mv) = amv(a) else { return Val::Undef };
    if b.mv(mv).category == Category::Status {
        return Val::Undef;
    }
    let flinch = b.d.c.flinch;
    let am = b.mvm(mv);
    let secs = am.secondaries.get_or_insert_with(Vec::new);
    if secs.iter().any(|x| x.volatile_status == Some(flinch)) {
        return Val::Undef;
    }
    secs.push(HitEffectData { chance: Some(10), volatile_status: Some(flinch), kingsrock: true, ..Default::default() });
    Val::Undef
}
fn redcard_after(b: &mut Battle, a: &HArgs) -> Val {
    let t = tgt(a);
    let Some(s) = src(a) else { return Val::Undef };
    let Some(mv) = amv(a) else { return Val::Undef };
    if s == t || b.p(s).hp <= 0 || b.p(t).hp <= 0 || b.mv(mv).category == Category::Status {
        return Val::Undef;
    }
    if !b.p(s).is_active || b.can_switch(s.s as usize) == 0 || b.p(s).force_switch_flag || b.p(t).force_switch_flag {
        return Val::Undef;
    }
    if b.use_item(t, Some(s), EffRef::NONE) {
        let mref = EffRef { eff: b.mv(mv).eff, mv, part: 0 };
        if b.run_event(Ev::DragOut, Arg::Mon(s), Some(t), mref, Val::True).truthy() {
            b.pm(s).force_switch_flag = true;
        }
    }
    Val::Undef
}

fn category_bp(b: &mut Battle, a: &HArgs, cat: Category) -> Val {
    if let Some(mv) = amv(a) {
        if b.mv(mv).category == cat {
            b.chain_modify(4505.0, 4096.0);
        }
    }
    Val::Undef
}
fn muscleband_bp(b: &mut Battle, a: &HArgs) -> Val {
    category_bp(b, a, Category::Physical)
}
fn wiseglasses_bp(b: &mut Battle, a: &HArgs) -> Val {
    category_bp(b, a, Category::Special)
}
fn airballoon_start(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    if !b.ignoring_item(m) && !b.field.pseudo_weather.iter().any(|x| b.eff(x.0).id == "gravity") {
        add!(b, "-item", m, "Air Balloon");
    }
    Val::Undef
}
fn pop_balloon(b: &mut Battle, m: Mon) {
    add!(b, "-enditem", m, "Air Balloon");
    let empty = b.d.c.empty;
    let item = b.p(m).item;
    b.pm(m).item = empty;
    Battle::clear_state(&mut b.pm(m).item_state, empty);
    b.run_event(Ev::AfterUseItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item));
}
fn airballoon_hit(b: &mut Battle, a: &HArgs) -> Val {
    let m = tgt(a);
    pop_balloon(b, m);
    Val::Undef
}
fn airballoon_after_sub(b: &mut Battle, a: &HArgs) -> Val {
    if b.etype(a.effect) == EffType::Move {
        let m = tgt(a);
        pop_balloon(b, m);
    }
    Val::Undef
}

fn shellbell_after(b: &mut Battle, a: &HArgs) -> Val {
    let u = tgt(a);
    let Some(mv) = amv(a) else { return Val::Undef };
    let td = b.mv(mv).total_damage;
    if td > 0.0 && !b.p(u).force_switch_flag {
        b.heal(td / 8.0, Some(u), None, EffRef::NONE);
    }
    Val::Undef
}
