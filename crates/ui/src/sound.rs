//! Event-driven UI sound cues. The application installs the platform player;
//! widgets only describe what the user did.

use egui::{Context, Id};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Tap,
    ToggleOn,
    ToggleOff,
    Expand,
    TileExpand,
    Collapse,
    Media,
    Scrub,
}

pub type Player = fn(Cue);

fn player_id() -> Id {
    Id::new("nook_ui_sound_player")
}

fn played_id() -> Id {
    Id::new("nook_ui_sound_played_this_frame")
}

fn enabled_id() -> Id {
    Id::new("nook_ui_sound_enabled")
}

fn scrub_id() -> Id {
    Id::new("nook_ui_sound_scrub_time")
}

/// Registers the app's platform sound player in this egui context.
pub fn install(ctx: &Context, player: Player) {
    ctx.data_mut(|data| data.insert_temp(player_id(), player));
}

/// Sets whether UI cues may play. The application syncs this from its saved
/// settings before drawing widgets each frame.
pub fn set_enabled(ctx: &Context, enabled: bool) {
    ctx.data_mut(|data| data.insert_temp(enabled_id(), enabled));
}

/// Starts a cue and marks the current frame as handled to prevent a generic
/// click cue from playing over this more specific sound.
pub fn play(ctx: &Context, cue: Cue) {
    if !ctx.data(|data| data.get_temp::<bool>(enabled_id()).unwrap_or(true)) {
        return;
    }
    let player = ctx.data(|data| data.get_temp::<Player>(player_id()));
    if let Some(player) = player {
        player(cue);
        ctx.data_mut(|data| data.insert_temp(played_id(), true));
    }
}

/// Plays a soft movement tick at most once every 65 ms for a given slider.
pub fn scrub(ctx: &Context, id: Id) {
    let now = ctx.input(|input| input.time);
    let key = scrub_id().with(id);
    let last = ctx.data(|data| data.get_temp::<f64>(key).unwrap_or(f64::NEG_INFINITY));
    if now - last >= 0.065 {
        play(ctx, Cue::Scrub);
        ctx.data_mut(|data| data.insert_temp(key, now));
    }
}

pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(played_id(), false));
}

pub fn played(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(played_id()).unwrap_or(false))
}
