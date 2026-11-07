//! The exact engine (M4): a line-by-line port of Pokemon Showdown's simulator for the
//! Champions VGC format, so that the same seed and the same choices give the same battle.

pub mod actions;
pub mod battle;
pub mod data;
pub mod events;
pub mod flow;
pub mod handlers;
pub mod pokemon;
pub mod prng;
pub mod state;
pub mod val;

/// Load the data (dex.json + effects.json), attach the handler implementations, and keep it
/// for the life of the program.
pub fn load_data(dex_path: &str, effects_path: &str) -> Result<&'static data::Data, String> {
    let mut d = data::Data::load(dex_path, effects_path)?;
    d.bind_handlers(handlers::lookup);
    Ok(Box::leak(Box::new(d)))
}
pub mod policy;
pub mod search;
