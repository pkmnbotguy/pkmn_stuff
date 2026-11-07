//! Every event name the engine fires (Showdown's runEvent / singleEvent / eachEvent / fieldEvent ids).

macro_rules! events {
    ($($name:ident),* $(,)?) => {
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u16)]
        pub enum Ev { $($name),* }
        pub const EV_NAMES: &[&str] = &[$(stringify!($name)),*];
    };
}

events! {
    Accuracy, AfterBoost, AfterEachBoost, AfterFaint, AfterHit, AfterMega, AfterMove, AfterMoveSecondary,
    AfterMoveSecondarySelf, AfterSetStatus, AfterSubDamage, AfterTakeItem, AfterTerastallization, AfterUseItem,
    Attract, BasePower, BattleStart, BeforeFaint, BeforeMove, BeforeSwitchIn, BeforeSwitchOut, BeforeTurn,
    ChangeBoost, ChargeMove, CheckShow, Copy, CriticalHit, Damage, DamagingHit, DeductPP, DisableMove, DragOut,
    Eat, EatItem, Effectiveness, EmergencyExit, End, Faint, FieldEnd, FieldRestart, FieldStart, FieldResidual,
    Flinch, FoeMaybeTrapPokemon, FractionalPriority, Heal, Hit, HitField, HitProtect, HitSide, Immunity,
    Invulnerability, LockMove, MaybeTrapPokemon, ModifyAccuracy, ModifyAtk, ModifyBoost, ModifyCritRatio,
    ModifyDamage, ModifyDef, ModifyMove, ModifyPriority, ModifySecondaries, ModifySpA, ModifySpD, ModifySpe,
    ModifySpecies, ModifySTAB, ModifyTarget, ModifyType, ModifyWeight, MoveAborted, MoveFail, NegateImmunity,
    OverrideAction, PrepareHit, PseudoWeatherChange, RedirectTarget, Residual, Restart, SemiLockMove,
    SetAbility, SetStatus, SetWeather, SideConditionStart, SideEnd, SideRestart, SideStart, SideResidual,
    StallMove, Start, Swap, SwitchIn, SwitchOut, TakeItem, TerrainChange, TrapPokemon, Try, TryAddVolatile,
    TryBoost, TryEatItem, TryHeal, TryHit, TryHitField, TryHitSide, TryImmunity, TryMove, TryPrimaryHit,
    TryTerrain, Type, Update, Use, UseItem, UseMoveMessage, Weather, WeatherChange, WeatherModifyDamage,
    AnySwitchIn, SourceModifyDamage, AfterSwitchInSelf, BasePowerCallback, DamageCallback, BeforeMoveCallback,
    BeforeTurnCallback, PriorityChargeCallback, DurationCallback, Sleep, SwitchInPriority, EntryHazard,
}

impl Ev {
    pub fn name(self) -> &'static str {
        EV_NAMES[self as usize]
    }
    /// events usually run through eachEvent only get `on{E}` handlers (findEventHandlers' prefixedHandlers)
    pub fn prefixed(self) -> bool {
        !matches!(self, Ev::BeforeTurn | Ev::Update | Ev::Weather | Ev::WeatherChange | Ev::TerrainChange)
    }
}
