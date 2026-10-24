//! Pose review (#61): hold a key to look at the previous or next drawing of the target clip, let
//! go and the playhead and selection are back where they were. The review only moves the playhead
//! (the engine's `drawing.previousCel` / `drawing.nextCel`): it edits nothing, makes no undo step
//! and does not touch the artwork or the tool.
//!
//! * `drawing.reviewPrevious`, `drawing.reviewNext` `{active?, key?, skipBlank?}` show the pose
//!   before or after the one the playhead was on when the review began (switching from one to the
//!   other stays relative to that pose). `active: false` ends it. Bound to a key the review lasts
//!   as long as the key is down; from a command or the control channel until `drawing.reviewEnd`.
//! * It also ends, restoring the playhead and selection, on Escape and when the window loses focus.
//!   If something else moves the playhead meanwhile (a scrub, playback), the review is dropped and
//!   that move stands.

use egui::Context;
use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_time::Tick;

use crate::WaltCutApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Previous,
    Next,
}

impl Pose {
    fn id(self) -> &'static str {
        match self {
            Pose::Previous => "drawing.reviewPrevious",
            Pose::Next => "drawing.reviewNext",
        }
    }
    fn nav(self) -> &'static str {
        match self {
            Pose::Previous => "drawing.previousCel",
            Pose::Next => "drawing.nextCel",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Pose::Previous => "previous",
            Pose::Next => "next",
        }
    }
}

/// A review in progress.
#[derive(Clone, Debug)]
pub struct Review {
    /// The pose being looked at.
    pub pose: Pose,
    /// Where the playhead was when the review began.
    origin: Tick,
    selection: Vec<ClipId>,
    /// The playhead the review itself last set; anything else is somebody else's move.
    shown: Tick,
    /// Held by a key (lasts while it is down) rather than by a command.
    key: bool,
}

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.reviewPrevious" | "drawing.reviewNext" => Some(
            r#"{"active":bool?,"skipBlank":bool?}  show the previous / next drawing until `active:false`, `drawing.reviewEnd`, Escape or loss of focus (bound to a key: while it is held); the playhead and selection come back"#,
        ),
        "drawing.reviewEnd" => Some("{}  end a review and restore the playhead and selection"),
        _ => None,
    }
}

/// `inspect` for the control channel.
pub fn state(app: &WaltCutApp) -> Value {
    match &app.review {
        Some(r) => json!({"active": true, "pose": r.pose.name(), "origin": r.origin.0, "key": r.key}),
        None => json!({"active": false}),
    }
}

/// End the review, putting the playhead and selection back when `restore`.
fn end(app: &mut WaltCutApp, restore: bool) -> bool {
    let Some(r) = app.review.take() else { return false };
    if restore {
        app.session.set_playhead(r.origin);
        app.session.state.selection = r.selection;
    }
    true
}

/// The review commands.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if id == "drawing.reviewEnd" {
        return Some(Ok(json!({"ended": end(app, true)})));
    }
    let pose = if id == Pose::Previous.id() { Pose::Previous } else { Pose::Next };
    let active = match params.get("active") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Some(Err("`active` must be true or false".into())),
    };
    if !active {
        // only the pose being shown can be let go of
        let shown = app.review.as_ref().is_some_and(|r| r.pose == pose);
        return Some(Ok(json!({"ended": shown && end(app, true)})));
    }
    let skip_blank = match params.get("skipBlank") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Some(Err("`skipBlank` must be true or false".into())),
    };
    let key = params.get("key").and_then(Value::as_bool).unwrap_or(false);
    // always relative to where the review began
    let (origin, selection, started) = match &app.review {
        Some(r) => (r.origin, r.selection.clone(), false),
        None => (app.session.playhead(), app.session.state.selection.clone(), true),
    };
    if !started {
        app.session.set_playhead(origin);
    }
    let result = app.session.execute(pose.nav(), json!({"skipBlank": skip_blank})).map_err(|e| e.to_string());
    match result {
        Ok(mut v) => {
            let shown = app.session.playhead();
            let kept = app.review.as_ref().is_some_and(|r| r.key) || key;
            app.review = Some(Review { pose, origin, selection, shown, key: kept });
            v["review"] = state(app);
            Some(Ok(v))
        }
        Err(e) => {
            // no such pose: stay on the original frame; a review already running keeps running
            if !started {
                app.session.set_playhead(origin);
            } else {
                app.review = None;
            }
            Some(Err(e))
        }
    }
}

/// End a review that no longer applies. Called every frame before shortcuts are handled.
pub fn release_stale_review(app: &mut WaltCutApp, ctx: &Context) {
    let Some(r) = &app.review else { return };
    // the loss itself, not the state: a window driven over the control channel is never focused
    let focus_lost = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let key_up = r.key && !app.bindings.iter().any(|(_, k, id, _)| id == r.pose.id() && ctx.input(|i| i.key_down(*k)));
    let moved_elsewhere = app.session.playhead() != r.shown || app.playback.playing;
    if moved_elsewhere {
        // a scrub or playback took over: that move stands
        end(app, false);
    } else if focus_lost || escape || key_up {
        end(app, true);
    }
}
