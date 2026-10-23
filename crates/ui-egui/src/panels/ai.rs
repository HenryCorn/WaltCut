//! The AI Restyle window (Graphics and Titles ▸ AI Restyle…): re-style the Program frame, the
//! In/Out range or the selected clip with an image model (`ai.restyle`), and pick the provider
//! (`ai.setup`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use waltcut_engine::ai::CONFIRM_ABOVE_FRAMES;

use crate::WaltCutApp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestyleRange {
    #[default]
    Playhead,
    InOut,
    Clip,
}

impl RestyleRange {
    const ALL: [RestyleRange; 3] = [RestyleRange::Playhead, RestyleRange::InOut, RestyleRange::Clip];
    fn label(self) -> &'static str {
        match self {
            RestyleRange::Playhead => "Frame at playhead",
            RestyleRange::InOut => "In to Out",
            RestyleRange::Clip => "Selected clip",
        }
    }
    fn param(self) -> &'static str {
        match self {
            RestyleRange::Playhead => "playhead",
            RestyleRange::InOut => "inout",
            RestyleRange::Clip => "clip",
        }
    }
}

/// AI Restyle window state (saved with the UI state, except the key being typed).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiWindow {
    pub open: bool,
    pub prompt: String,
    pub negative: String,
    pub strength: f32,
    pub seed: u64,
    pub range: RestyleRange,
    pub every: u32,
    /// Ticked to send more frames than `CONFIRM_ABOVE_FRAMES`; asked again for every request.
    #[serde(skip)]
    pub confirm: bool,
    #[serde(skip)]
    pub key_input: String,
    #[serde(skip)]
    pub job: Option<u64>,
}

impl Default for AiWindow {
    fn default() -> Self {
        Self {
            open: false,
            prompt: "hand-painted watercolor illustration, soft paper texture".into(),
            negative: "blurry, low quality, text, watermark".into(),
            strength: 0.55,
            seed: 1234,
            range: RestyleRange::Playhead,
            every: 2,
            confirm: false,
            key_input: String::new(),
            job: None,
        }
    }
}

/// Drawing assists on the same image-to-image call: (label, prompt, avoid, strength). Low strength
/// keeps the drawing's composition.
const ASSIST_PRESETS: [(&str, &str, &str, f32); 3] = [
    ("Watercolor", "hand-painted watercolor illustration, soft paper texture", "blurry, low quality, text, watermark", 0.55),
    (
        "Sketch cleanup (rough to clean line art)",
        "clean black ink line art, smooth confident lines, white background, no shading",
        "sketchy, messy lines, colour, shading, blurry, text",
        0.45,
    ),
    ("Flat colour fill", "flat colour cel animation, solid fills, clean outlines, no gradients", "gradients, shading, texture, blurry, text, watermark", 0.40),
];

/// How many frames a request would send.
fn frame_count(app: &WaltCutApp, w: &AiWindow) -> Option<u64> {
    let seq = app.session.active_sequence()?;
    let fd = seq.settings.frame_rate.frame_duration().0.max(1);
    let span = match w.range {
        RestyleRange::Playhead => return Some(1),
        RestyleRange::InOut => seq.mark_out?.0.saturating_sub(seq.mark_in?.0),
        RestyleRange::Clip => {
            let c = app.session.state.selection.first()?;
            seq.find_item(*c)?.1.duration.0
        }
    };
    let step = fd.saturating_mul(w.every.max(1) as i64);
    Some(((span + step - 1) / step).max(1) as u64)
}

pub fn window(app: &mut WaltCutApp, ctx: &egui::Context) {
    if !app.ui.ai.open {
        return;
    }
    let mut open = true;
    let status = app.session.execute("ai.status", json!({})).unwrap_or(Value::Null);
    egui::Window::new("AI Restyle").open(&mut open).default_width(380.0).resizable(true).show(ctx, |ui| {
        let r = ui.collapsing("Provider", |ui| {
            ui.label(format!("{} · model {}", status["providerLabel"].as_str().unwrap_or("?"), status["endpoint"].as_str().unwrap_or("?")));
            ui.horizontal(|ui| {
                for (name, label) in [("fal", "fal.ai (cloud)"), ("local", "Stable Diffusion (local, coming soon)")] {
                    let on = status["provider"].as_str() == Some(if name == "fal" { "fal" } else { "localStableDiffusion" });
                    if ui.selectable_label(on, label).clicked() {
                        run(app, "ai.setup", json!({"provider": name}));
                    }
                }
            });
            let endpoint = status["endpoint"].as_str().unwrap_or_default().to_string();
            let models = status["models"].as_array().cloned().unwrap_or_default();
            let label_of = |m: &Value| m["label"].as_str().unwrap_or_default().to_string();
            let current = models.iter().find(|m| m["endpoint"].as_str() == Some(endpoint.as_str())).map_or(endpoint.clone(), label_of);
            egui::ComboBox::from_label("Model").selected_text(current).show_ui(ui, |ui| {
                for m in &models {
                    let ep = m["endpoint"].as_str().unwrap_or_default();
                    if ui.selectable_label(endpoint == ep, label_of(m)).clicked() {
                        run(app, "ai.setup", json!({"endpoint": ep}));
                    }
                }
            });
            let key = status["key"].as_str().unwrap_or("missing");
            ui.label(match key {
                "saved" => "API key: saved",
                "environment" => "API key: from FAL_KEY",
                _ => "API key: missing (get one at fal.ai/dashboard/keys)",
            });
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut app.ui.ai.key_input).password(true).hint_text("paste fal.ai key").desired_width(220.0));
                app.auto.add("ai.key", r.rect, "fal.ai API key");
                if ui.button("Save key").clicked() && !app.ui.ai.key_input.trim().is_empty() {
                    let k = std::mem::take(&mut app.ui.ai.key_input);
                    run(app, "ai.setup", json!({"apiKey": k}));
                }
            });
            if !status["cloud"].as_bool().unwrap_or(false) {
                ui.colored_label(app.tokens.danger, "This build has no cloud AI.");
            }
        });
        app.auto.add("ai.provider", r.header_response.rect, "provider");
        ui.separator();
        let w = &mut app.ui.ai;
        egui::ComboBox::from_label("Preset").selected_text("Choose…").show_ui(ui, |ui| {
            for (label, prompt, negative, strength) in ASSIST_PRESETS {
                if ui.selectable_label(false, label).clicked() {
                    (w.prompt, w.negative, w.strength) = (prompt.into(), negative.into(), strength);
                }
            }
        });
        ui.label("Style prompt");
        ui.add(egui::TextEdit::multiline(&mut w.prompt).desired_rows(3).desired_width(f32::INFINITY));
        ui.label("Avoid");
        ui.add(egui::TextEdit::singleline(&mut w.negative).desired_width(f32::INFINITY));
        ui.add(egui::Slider::new(&mut w.strength, 0.05..=1.0).text("Strength (higher repaints more)"));
        ui.horizontal(|ui| {
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut w.seed));
            ui.label("same seed = steadier look between frames");
        });
        egui::ComboBox::from_label("Frames").selected_text(w.range.label()).show_ui(ui, |ui| {
            for r in RestyleRange::ALL {
                ui.selectable_value(&mut w.range, r, r.label());
            }
        });
        if w.range != RestyleRange::Playhead {
            ui.horizontal(|ui| {
                ui.label("Animate on");
                for (n, label) in [(2, "every 2nd"), (3, "every 3rd"), (4, "every 4th")] {
                    ui.selectable_value(&mut w.every, n, label);
                }
            });
            if !(2..=4).contains(&w.every) {
                w.every = 2;
            }
        }
        let snapshot = app.ui.ai.clone();
        let n = frame_count(app, &snapshot);
        ui.label(match n {
            Some(1) => "1 frame will be sent.".to_string(),
            Some(n) => format!("{n} frames will be sent (each one is billed by a cloud provider)."),
            None => "Set In and Out, or select a clip.".to_string(),
        });
        let busy = snapshot
            .job
            .and_then(|id| app.session.jobs.iter().find(|j| j.id == id))
            .is_some_and(|j| !j.progress.finished.load(std::sync::atomic::Ordering::Relaxed));
        let big = n.is_some_and(|n| n > CONFIRM_ABOVE_FRAMES as u64);
        if big {
            ui.checkbox(&mut app.ui.ai.confirm, format!("I understand this sends {} billed frames", n.unwrap_or(0)));
        }
        let allowed = !big || app.ui.ai.confirm;
        let go = ui.add_enabled(!busy && n.is_some() && allowed && status["ready"].as_bool().unwrap_or(false), egui::Button::new("Restyle"));
        app.auto.add("ai.restyle", go.rect, "Restyle");
        if go.clicked() {
            let p = json!({
                "prompt": snapshot.prompt,
                "negativePrompt": snapshot.negative,
                "strength": snapshot.strength,
                "seed": snapshot.seed,
                "range": snapshot.range.param(),
                "every": snapshot.every,
                "confirm": big && snapshot.confirm,
            });
            match app.session.execute("ai.restyle", p) {
                Ok(r) => {
                    app.ui.ai.job = r["job"].as_u64();
                    app.ui.ai.confirm = false;
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        if let Some(j) = snapshot.job.and_then(|id| app.session.jobs.iter().find(|j| j.id == id)) {
            let p = &j.progress;
            ui.add(egui::ProgressBar::new(p.fraction()).text(p.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()));
            if let Some(e) = p.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
                ui.colored_label(app.tokens.danger, e);
            }
            if busy {
                if ui.button("Cancel").clicked() {
                    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
    });
    app.ui.ai.open = open;
}

fn run(app: &mut WaltCutApp, cmd: &str, p: Value) {
    if let Err(e) = app.session.execute(cmd, p) {
        app.ui.status = e.to_string();
    }
}
