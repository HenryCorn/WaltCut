use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use filmcraft_genai::{GenError, Picture, RestyleRequest, Restyler};
use serde_json::json;

use crate::Session;

/// Answers every frame with a 4×4 red PNG; fails from frame `fail_from` on.
struct Fake {
    calls: AtomicUsize,
    fail_from: usize,
}

impl Restyler for Fake {
    fn name(&self) -> &str {
        "fake"
    }
    fn restyle(&self, picture: &Picture, req: &RestyleRequest) -> filmcraft_genai::Result<Vec<u8>> {
        assert!(picture.png.starts_with(b"\x89PNG"), "the frame is sent as PNG");
        assert!(picture.width > 0 && picture.height > 0);
        assert_eq!(req.prompt, "oil painting");
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n >= self.fail_from {
            return Err(GenError::Provider("out of credit".into()));
        }
        Ok(filmcraft_export::encode_png([255, 0, 0, 255].repeat(16), 4, 4).unwrap())
    }
}

fn session(fail_from: usize) -> (Session, Arc<Fake>) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "AI", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from });
    s.restyler_override = Some(fake.clone());
    (s, fake)
}

fn ai_track(s: &Session) -> &filmcraft_project::Track {
    s.active_sequence().unwrap().video_tracks.iter().find(|t| t.name.starts_with("AI Restyle")).expect("an AI Restyle track")
}

#[test]
fn restyled_range_lands_on_its_own_track_held_on_twos() {
    let (mut s, fake) = session(usize::MAX);
    s.execute("drawing.new", json!({"seconds": 1})).unwrap();
    s.execute("markers.markIn", json!({"frame": 0})).unwrap();
    s.execute("markers.markOut", json!({"frame": 7})).unwrap();
    let r = s.execute("ai.restyle", json!({"prompt": "oil painting", "range": "inout", "every": 2, "wait": true})).unwrap();
    let frames = r["frames"].as_u64().unwrap();
    assert_eq!(fake.calls.load(Ordering::SeqCst) as u64, frames);
    let rate = s.active_sequence().unwrap().settings.frame_rate;
    let tr = ai_track(&s);
    assert_eq!(tr.items.len() as u64, frames);
    let starts: Vec<i64> = tr.items.iter().map(|i| rate.frame_at(i.start)).collect();
    assert_eq!(starts, (0..frames as i64).map(|k| k * 2).collect::<Vec<_>>());
    assert!(tr.items.iter().all(|i| i.scale_to_frame && i.duration == rate.tick_of(2)));
    // the import is one undo step away from the result
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.active_sequence().unwrap().video_tracks.iter().any(|t| t.name.starts_with("AI Restyle")));
}

#[test]
fn a_failing_provider_keeps_the_frames_it_finished() {
    let (mut s, _) = session(1);
    s.execute("playhead.set", json!({"frame": 0})).unwrap();
    s.execute("markers.markIn", json!({"frame": 0})).unwrap();
    s.execute("markers.markOut", json!({"frame": 5})).unwrap();
    let r = s.execute("ai.restyle", json!({"prompt": "oil painting", "range": "inout", "every": 1, "wait": true})).unwrap();
    let job = r["job"].as_u64().unwrap();
    let j = s.jobs.iter().find(|j| j.id == job).unwrap();
    assert!(j.progress.error.lock().unwrap().as_deref().is_some_and(|e| e.contains("out of credit")));
    assert_eq!(ai_track(&s).items.len(), 1);
}

#[test]
fn bad_requests_are_refused_before_anything_is_billed() {
    let (mut s, fake) = session(usize::MAX);
    for p in [
        json!({"prompt": ""}),
        json!({"prompt": "oil painting", "strength": 3}),
        json!({"prompt": "oil painting", "range": "sideways"}),
        json!({"prompt": "oil painting", "range": "inout"}),
        json!({"prompt": "oil painting", "range": "clip"}),
    ] {
        assert!(s.execute("ai.restyle", p.clone()).is_err(), "{p}");
    }
    // too many frames for one request
    s.execute("markers.markIn", json!({"frame": 0})).unwrap();
    s.execute("markers.markOut", json!({"frame": 100})).unwrap();
    assert!(s.execute("ai.restyle", json!({"prompt": "oil painting", "range": "inout", "every": 1, "maxFrames": 10})).is_err());
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn setup_validates_and_status_never_shows_the_key() {
    let mut s = Session::default();
    assert!(s.execute("ai.setup", json!({"endpoint": "../../evil"})).is_err());
    assert!(s.execute("ai.setup", json!({"provider": "dalle"})).is_err());
    let st = s.execute("ai.setup", json!({"provider": "fal", "apiKey": "sk-very-secret"})).unwrap();
    assert_eq!(st["key"], "saved");
    assert_eq!(st["ready"], true);
    assert!(!st.to_string().contains("very-secret"));
    assert!(!serde_json::to_string(&s.prefs).unwrap().contains("very-secret"), "the key is not in the preferences file");
    let st = s.execute("ai.setup", json!({"provider": "local"})).unwrap();
    assert_eq!(st["ready"], false, "local Stable Diffusion is not implemented yet");
    s.execute("file.newSequence", json!({"name": "AI", "fps": 24, "width": 64, "height": 64})).unwrap();
    let e = s.execute("ai.restyle", json!({"prompt": "x"})).unwrap_err().to_string();
    assert!(e.contains("not available yet"), "{e}");
}
