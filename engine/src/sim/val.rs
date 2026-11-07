//! JavaScript-like values for event relay variables and handler results. Showdown's event
//! system distinguishes `undefined`, `null`, `false`, `''` (NOT_FAIL), `0` and numbers, and
//! handlers return all of them; the engine has to keep those distinctions to behave the same.

use super::data::{EffId, SparseBoosts, TypeId};

/// A Pokemon: side and index in that side's team (stable for the whole battle).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mon {
    pub s: u8,
    pub i: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    Undef,
    Null,
    False,
    True,
    /// '' (NOT_FAIL)
    Empty,
    Num(f64),
    Mon(Mon),
    /// an effect / condition / move id (status for SetStatus, move id for LockMove...)
    Eff(EffId),
    /// a move id given as a string (LockMove 'recharge')
    MoveId(u16),
    Boosts(SparseBoosts),
    /// up to 3 types (Type event)
    Types([TypeId; 3], u8),
    /// a type name (Effectiveness / Immunity events pass a type)
    Type(TypeId),
    /// an immunity key that is not a type ('brn', 'sandstorm', 'powder', 'trapped'...)
    Key(&'static str),
    /// an active move (ModifyMove / ModifyType relay the move itself)
    Move(u16),
    /// RedirectTarget relays a target: (pokemon)
    Side(u8),
    Field,
}

impl Val {
    #[inline]
    pub fn truthy(&self) -> bool {
        match self {
            Val::Undef | Val::Null | Val::False | Val::Empty => false,
            Val::Num(x) => *x != 0.0 && !x.is_nan(),
            _ => true,
        }
    }
    #[inline]
    pub fn is_undef(&self) -> bool {
        matches!(self, Val::Undef)
    }
    #[inline]
    pub fn num(&self) -> f64 {
        match self {
            Val::Num(x) => *x,
            Val::True => 1.0,
            _ => 0.0,
        }
    }
    pub fn from_bool(b: bool) -> Val {
        if b { Val::True } else { Val::False }
    }
    /// `typeof v === 'number'`
    pub fn is_num(&self) -> bool {
        matches!(self, Val::Num(_))
    }
    pub fn from_json(v: &serde_json::Value) -> Val {
        match v {
            serde_json::Value::Null => Val::Null,
            serde_json::Value::Bool(true) => Val::True,
            serde_json::Value::Bool(false) => Val::False,
            serde_json::Value::Number(n) => Val::Num(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) if s.is_empty() => Val::Empty,
            // string handler values (onLockMove: 'recharge') are resolved by the engine
            serde_json::Value::String(_) => Val::Key("string"),
            _ => Val::Undef,
        }
    }
}

/// Showdown's `combineResults` priority: undefined < '' < null < boolean < number
pub fn combine_results(left: Val, right: Val) -> Val {
    fn rank(v: &Val) -> i32 {
        match v {
            Val::Undef => 0,
            Val::Empty => 1,
            Val::Null => 2,
            Val::True | Val::False => 3,
            Val::Num(_) => 4,
            _ => 3,
        }
    }
    if rank(&left) > rank(&right) {
        left
    } else if left.truthy() && !right.truthy() && !matches!(right, Val::Num(x) if x == 0.0) {
        left
    } else if let (Val::Num(a), Val::Num(b)) = (left, right) {
        Val::Num(a + b)
    } else {
        right
    }
}
