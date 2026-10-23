//! egui glue for the spring animator: id-keyed springs stored in egui memory,
//! a global reduced-motion switch, and an icon crossfade helper.
//!
//! Every helper requests a repaint only while something is still moving, so
//! a settled UI costs nothing.

use crate::spring::{Spring, SpringParams};
use egui::{Context, Id};

/// Global motion preference, mirrored from the Windows "animation effects"
/// setting.
#[derive(Clone, Copy, Debug, Default)]
pub struct Motion {
    pub reduced: bool,
}

pub fn set_motion(ctx: &Context, motion: Motion) {
    ctx.data_mut(|d| d.insert_temp(Id::NULL.with("nook-motion"), motion));
}

pub fn motion(ctx: &Context) -> Motion {
    ctx.data(|d| d.get_temp(Id::NULL.with("nook-motion")))
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
struct SpringSlot {
    spring: Spring,
    last_time: f64,
}

/// Frame delta for a spring last stepped at `last_time`. After an idle gap
/// the first step uses the predicted frame time instead of the whole gap.
fn frame_dt(ctx: &Context, last_time: f64) -> f32 {
    ctx.input(|i| {
        let gap = (i.time - last_time) as f32;
        if (0.0..0.1).contains(&gap) {
            gap
        } else {
            i.predicted_dt
        }
    })
}

/// Animates towards `target` with a spring and returns the current value.
/// The first call for an `id` starts at `target`.
pub fn spring(ctx: &Context, id: Id, target: f32, params: SpringParams) -> f32 {
    spring_from(ctx, id, target, target, params)
}

/// Like [`spring`], but the first call starts at `initial`.
pub fn spring_from(ctx: &Context, id: Id, initial: f32, target: f32, params: SpringParams) -> f32 {
    let now = ctx.input(|i| i.time);
    let mut slot = ctx
        .data(|d| d.get_temp::<SpringSlot>(id))
        .unwrap_or(SpringSlot {
            spring: Spring::new(initial, params),
            last_time: now,
        });
    slot.spring.params = params;
    slot.spring.set_target(target);
    if motion(ctx).reduced {
        slot.spring.snap(target);
    } else {
        let dt = frame_dt(ctx, slot.last_time);
        if slot.spring.step(dt) {
            ctx.request_repaint();
        }
    }
    slot.last_time = now;
    let value = slot.spring.value;
    ctx.data_mut(|d| d.insert_temp(id, slot));
    value
}

/// Two springs moving a point (or size) toward `target`; `snap` jumps
/// there with no motion. The first call for an `id` starts at `target`.
pub fn spring_vec2(ctx: &Context, id: Id, target: egui::Vec2, snap: bool) -> egui::Vec2 {
    let params = SpringParams::with_overshoot(360.0, 0.05);
    let now = ctx.input(|i| i.time);
    let (mut x, mut y, last) = ctx
        .data(|d| d.get_temp::<(Spring, Spring, f64)>(id))
        .unwrap_or((
            Spring::new(target.x, params),
            Spring::new(target.y, params),
            now,
        ));
    x.set_target(target.x);
    y.set_target(target.y);
    if snap || motion(ctx).reduced {
        x.snap(target.x);
        y.snap(target.y);
    } else {
        let dt = frame_dt(ctx, last);
        if x.step(dt) | y.step(dt) {
            ctx.request_repaint();
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, (x, y, now)));
    egui::vec2(x.value, y.value)
}

/// Linear 0..1 tween over `duration` seconds towards `on`. Used where a fixed
/// duration is part of the spec (content fade, icon crossfade).
pub fn tween(ctx: &Context, id: Id, on: bool, duration: f32) -> f32 {
    let duration = if motion(ctx).reduced { 0.0 } else { duration };
    ctx.animate_bool_with_time(id, on, duration)
}

/// Remembers the last icon shown under `id` and returns `(previous, t)`,
/// where `t` runs 0 -> 1 over 120 ms after the glyph changes. Draw the
/// previous glyph with alpha `1 - t` and the current one with `t`.
pub fn icon_crossfade(ctx: &Context, id: Id, glyph: &'static str) -> (Option<&'static str>, f32) {
    const DURATION: f64 = 0.12;
    let now = ctx.input(|i| i.time);
    let key = id.with("icon-xfade");
    let (current, previous, since) = ctx
        .data(|d| d.get_temp::<(&'static str, Option<&'static str>, f64)>(key))
        .unwrap_or((glyph, None, f64::NEG_INFINITY));
    let (previous, since) = if current != glyph {
        (Some(current), now)
    } else {
        (previous, since)
    };
    let mut t = ((now - since) / DURATION).clamp(0.0, 1.0) as f32;
    if motion(ctx).reduced {
        t = 1.0;
    }
    if t < 1.0 {
        ctx.request_repaint();
    }
    let previous = if t < 1.0 { previous } else { None };
    ctx.data_mut(|d| d.insert_temp(key, (glyph, previous, since)));
    (previous, t)
}

/// Cubic ease-out, for stagger reveals.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}
