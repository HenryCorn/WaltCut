//! Linked exposures (#67): hold, independent duplicate and link are three different operations; an
//! edit of one linked exposure is an edit of all of them; make independent, delete, undo, save and
//! load, the locked track and a refusal keep the artwork.
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_project::drawing::Drawing;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("link-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Link", "fps": 24, "width": 160, "height": 90, "video": 1, "audio": 1})).unwrap();
    let r = s.execute("drawing.new", json!({"seconds": 4})).unwrap();
    (s, ClipId(r["clip"].as_u64().unwrap()))
}

fn drawing(s: &Session, clip: ClipId) -> Drawing {
    let seq = s.active_sequence().unwrap();
    crate::drawing::drawing_of(s, seq, clip).unwrap().1.clone()
}

fn dash(color: &str, y: f64) -> Value {
    json!({"brush": "Pen", "color": color, "size": 4.0, "points": [[20.0, y], [140.0, y]]})
}

/// An open-eyes cel on frame 0, a closed-eyes cel on frame 4 and the open eyes linked to frames
/// 8 and 12: a blink cycle.
fn blink() -> (Session, ClipId) {
    let (mut s, clip) = session();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": dash("#000000", 30.0)})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 4, "cel": "new", "stroke": dash("#0000ff", 60.0)})).unwrap();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 8})).unwrap();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 12})).unwrap();
    (s, clip)
}

fn links(d: &Drawing) -> Vec<(i64, Option<u32>, usize)> {
    d.cels.iter().map(|c| (c.frame, c.link, c.strokes.len())).collect()
}

fn preview(s: &Session) -> Vec<u8> {
    s.render_program(1.0).unwrap().to_rgba8()
}

fn at_frame(s: &mut Session, f: i64) -> Vec<u8> {
    s.execute("playhead.set", json!({"frame": f})).unwrap();
    preview(s)
}

fn snapshot(s: &Session, clip: ClipId) -> (u64, usize, usize, Drawing) {
    (s.revision, s.history.undo.len(), s.history.redo.len(), drawing(s, clip))
}

#[test]
fn a_blink_cycle_edit_changes_every_linked_exposure_and_nothing_else() {
    let (mut s, clip) = blink();
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    let id = d.cels[0].link.unwrap();
    assert_eq!(links(&d), vec![(0, Some(id), 1), (4, None, 1), (8, Some(id), 1), (12, Some(id), 1)]);
    assert!(Arc::ptr_eq(&d.cels[0].strokes, &d.cels[2].strokes) && Arc::ptr_eq(&d.cels[0].strokes, &d.cels[3].strokes));
    let (closed, open_before) = (at_frame(&mut s, 4), at_frame(&mut s, 8));
    assert_eq!(at_frame(&mut s, 0), open_before, "linked exposures show the same pixels");
    // draw an eyelash on the exposure at frame 12: the exposures at 0 and 8 get it, the closed eyes do not
    let r = s.execute("drawing.addStroke", json!({"frame": 12, "stroke": dash("#ff0000", 45.0)})).unwrap();
    assert_eq!((r["cel"].clone(), r["strokes"].clone(), r["linked"].clone()), (json!(12), json!(2), json!(3)));
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    assert_eq!(links(&d), vec![(0, Some(id), 2), (4, None, 1), (8, Some(id), 2), (12, Some(id), 2)]);
    let (a, b, c) = (at_frame(&mut s, 0), at_frame(&mut s, 8), at_frame(&mut s, 12));
    assert_ne!(a, open_before);
    assert!(a == b && b == c, "every linked exposure changed");
    assert_eq!(at_frame(&mut s, 4), closed, "the unlinked cel did not");
    // an eraser, in paint order, is shared too
    s.execute("drawing.addStroke", json!({"frame": 8, "stroke": {"brush": "Eraser", "size": 12, "points": [[80.0, 20.0], [80.0, 40.0]]}})).unwrap();
    assert_eq!(links(&drawing(&s, clip))[0].2, 3);
    assert_eq!(at_frame(&mut s, 0), at_frame(&mut s, 12));
    // clearing one exposure of a link clears them all, and says how many changed
    let r = s.execute("drawing.clearCel", json!({"frame": 8})).unwrap();
    assert_eq!(r["linked"], 3);
    assert_eq!(links(&drawing(&s, clip)).iter().filter(|c| c.2 == 0).count(), 3);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(links(&drawing(&s, clip))[3].2, 3, "undo restores every exposure");
}

#[test]
fn hold_duplicate_and_link_are_three_different_operations() {
    let (mut s, clip) = session();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": dash("#000000", 30.0)})).unwrap();
    // hold: one cel, longer: nothing is linked
    let d = drawing(&s, clip);
    assert!(d.cels.len() == 1 && d.cel_at(3).map(|c| c.frame) == Some(0) && d.cels[0].link.is_none(), "a hold is one cel shown longer");
    // duplicate: a copy that is independent, even from a linked source
    s.execute("drawing.newCel", json!({"frame": 4, "copy": true})).unwrap();
    assert!(drawing(&s, clip).cels.iter().all(|c| c.link.is_none()), "a duplicate is not a link");
    s.execute("drawing.addStroke", json!({"frame": 4, "stroke": dash("#00ff00", 60.0)})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!((d.cels[0].strokes.len(), d.cels[1].strokes.len()), (1, 2), "editing a duplicate leaves the original alone");
    // link: shared
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 8})).unwrap();
    let d = drawing(&s, clip);
    assert!(d.cels[0].link.is_some() && d.cels[0].link == d.cels[2].link && d.cels[1].link.is_none());
    // a duplicate of a linked exposure is independent of the group
    s.execute("drawing.newCel", json!({"frame": 12, "copy": true})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 12, "stroke": dash("#ff00ff", 70.0)})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!((d.cels[3].link, d.cels[3].strokes.len(), d.cels[0].strokes.len(), d.cels[2].strokes.len()), (None, 2, 1, 1));
    // drawing with a copying cel mode on a linked exposure also starts an independent cel
    s.execute("drawing.addStroke", json!({"frame": 16, "cel": "copy", "stroke": dash("#112233", 20.0)})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!(d.cels.last().map(|c| (c.frame, c.link)), Some((16, None)));
    d.check_links().unwrap();
}

#[test]
fn make_independent_keeps_the_strokes_and_the_others_stay_linked() {
    let (mut s, clip) = blink();
    let undo = s.history.undo.len();
    let r = s.execute("drawing.makeIndependent", json!({"frame": 8})).unwrap();
    assert_eq!((r["cel"].clone(), r["remaining"].clone()), (json!(8), json!(2)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    let id = d.cels[0].link;
    assert!(id.is_some() && d.cels[3].link == id && d.cels[2].link.is_none());
    assert_eq!(d.cels[2].strokes.len(), 1, "it kept what it showed");
    // an edit of the independent cel no longer reaches the group, and the group's no longer reaches it
    s.execute("drawing.addStroke", json!({"frame": 8, "stroke": dash("#ff0000", 45.0)})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 12, "stroke": dash("#00ff00", 15.0)})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!(links(&d).iter().map(|c| c.2).collect::<Vec<_>>(), vec![2, 1, 2, 2]);
    assert_ne!(at_frame(&mut s, 8), at_frame(&mut s, 12));
    assert_eq!(at_frame(&mut s, 0), at_frame(&mut s, 12));
    // the last peer to leave dissolves the group: nothing is left linked to nothing
    s.execute("drawing.makeIndependent", json!({"frame": 0})).unwrap();
    let d = drawing(&s, clip);
    assert!(d.cels.iter().all(|c| c.link.is_none()), "{:?}", links(&d));
    d.check_links().unwrap();
    // refused: an unlinked cel
    let before = snapshot(&s, clip);
    let e = s.execute("drawing.makeIndependent", json!({"frame": 4})).unwrap_err();
    assert!(e.to_string().contains("not linked"), "{e}");
    assert_eq!(snapshot(&s, clip), before);
    // undo, redo
    s.execute("edit.undo", json!({})).unwrap();
    let d = drawing(&s, clip);
    assert!(d.cels[0].link.is_some() && d.cels[0].link == d.cels[3].link);
    d.check_links().unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert!(drawing(&s, clip).cels.iter().all(|c| c.link.is_none()));
}

#[test]
fn deleting_one_exposure_keeps_the_content_on_the_others() {
    let (mut s, clip) = blink();
    let shown = at_frame(&mut s, 12);
    s.execute("drawing.deleteCel", json!({"frame": 8})).unwrap();
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    assert_eq!(d.cels.iter().map(|c| c.frame).collect::<Vec<_>>(), vec![0, 4, 12]);
    assert!(d.cels[0].link.is_some() && d.cels[0].link == d.cels[2].link);
    assert_eq!(at_frame(&mut s, 12), shown, "the survivors still show the artwork");
    // deleting the original, not just a copy of it, is the same
    s.execute("drawing.deleteCel", json!({"frame": 0})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!(links(&d), vec![(4, None, 1), (12, None, 1)]);
    assert_eq!(at_frame(&mut s, 12), shown, "the last exposure was dissolved from its group and kept its strokes");
    // undo twice: the cycle is back and linked
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!(d.cels.iter().filter(|c| c.link.is_some()).count(), 3);
    d.check_links().unwrap();
    assert_eq!(at_frame(&mut s, 8), shown);
}

#[test]
fn fills_and_lasso_transforms_reach_every_peer() {
    let (mut s, clip) = session();
    let ring = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": [[40.0, 20.0], [120.0, 20.0], [120.0, 70.0], [40.0, 70.0], [40.0, 20.0]]});
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": ring})).unwrap();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 6})).unwrap();
    let empty = at_frame(&mut s, 6);
    // a fill made on the exposure at 6 is on the one at 0
    s.execute("drawing.fill", json!({"frame": 6, "point": [80.5, 45.5], "color": "#ff0000"})).unwrap();
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    assert_eq!((d.cels[0].strokes.len(), d.cels[1].strokes.len()), (2, 2));
    assert!(Arc::ptr_eq(&d.cels[0].strokes, &d.cels[1].strokes));
    let filled = at_frame(&mut s, 6);
    assert_ne!(filled, empty);
    assert_eq!(at_frame(&mut s, 0), filled);
    // recolouring and removing it reach both
    s.execute("drawing.fillEdit", json!({"frame": 0, "index": 1, "color": "#00ff00"})).unwrap();
    assert_ne!(at_frame(&mut s, 6), filled);
    assert_eq!(at_frame(&mut s, 0), at_frame(&mut s, 6));
    // the lasso transform (#66) moves the strokes of every linked exposure
    let before = at_frame(&mut s, 0);
    s.execute("drawing.transformStrokes", json!({"frame": 6, "strokes": [0], "dx": 10, "dy": 5})).unwrap();
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    assert!(Arc::ptr_eq(&d.cels[0].strokes, &d.cels[1].strokes));
    assert_ne!(at_frame(&mut s, 0), before);
    assert_eq!(at_frame(&mut s, 0), at_frame(&mut s, 6));
    s.execute("drawing.fillRemove", json!({"frame": 0, "index": 1})).unwrap();
    assert_eq!(drawing(&s, clip).cels.iter().map(|c| c.strokes.len()).collect::<Vec<_>>(), vec![1, 1]);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(at_frame(&mut s, 6), filled, "undo steps back through the shared edits");
    assert_eq!(at_frame(&mut s, 0), filled);
}

#[test]
fn a_linked_project_saves_the_artwork_once_and_loads_linked_without_aliasing_anything_else() {
    let (mut s, clip) = blink();
    // a second cel that merely looks like the linked one: it is a copy, not a link
    s.execute("drawing.newCel", json!({"frame": 16, "copy": true})).unwrap();
    let dir = Scratch::new("save");
    let path = dir.path("blink.waltcut");
    s.execute("file.save", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let compact: String = text.split_whitespace().collect();
    assert!(compact.contains("\"links\":[{\"id\":1,\"strokes\":["), "{compact}");
    assert_eq!(compact.matches("\"#000000\"").count() + compact.matches("[0.0,0.0,0.0,1.0]").count(), 2, "the linked artwork once, and the copy's own");
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    let (a, b) = (drawing(&s, clip), drawing(&t, clip));
    assert_eq!(a, b, "the same linked drawing comes back");
    b.check_links().unwrap();
    assert!(Arc::ptr_eq(&b.cels[0].strokes, &b.cels[2].strokes) && Arc::ptr_eq(&b.cels[0].strokes, &b.cels[3].strokes));
    assert!(!Arc::ptr_eq(&b.cels[0].strokes, &b.cels[4].strokes), "the copy of the same strokes is not aliased with the link");
    // an edit after loading reaches the group and only the group
    t.execute("drawing.addStroke", json!({"frame": 8, "stroke": dash("#ff0000", 45.0)})).unwrap();
    let c = drawing(&t, clip);
    assert_eq!(links(&c).iter().map(|c| c.2).collect::<Vec<_>>(), vec![2, 1, 2, 2, 1]);
    // and it saves again to the same document
    t.execute("file.save", json!({"path": dir.path("again.waltcut")})).unwrap();
    let mut u = Session::default();
    u.execute("file.open", json!({"path": dir.path("again.waltcut")})).unwrap();
    assert_eq!(drawing(&u, clip), c);
}

#[test]
fn the_preview_and_the_exported_still_agree_on_linked_exposures() {
    let (mut s, _) = blink();
    s.execute("drawing.addStroke", json!({"frame": 8, "stroke": dash("#ff8800", 45.0)})).unwrap();
    let dir = Scratch::new("export");
    for f in [0, 4, 8, 12, 15] {
        s.execute("playhead.set", json!({"frame": f})).unwrap();
        let png = dir.path(&format!("still-{f}.png"));
        s.execute("file.exportFrame", json!({"path": png})).unwrap();
        let exported = image::load_from_memory(&std::fs::read(&png).unwrap()).unwrap().to_rgba8();
        assert_eq!(exported.as_raw(), &preview(&s), "frame {f}: export and preview are the same pixels");
    }
}

#[test]
fn a_locked_track_and_every_bad_request_change_nothing() {
    let (mut s, clip) = blink();
    let before = snapshot(&s, clip);
    for (id, params) in [
        ("drawing.linkCel", json!({"from": 0, "frame": 8})),
        ("drawing.linkCel", json!({"from": 0, "frame": 0})),
        ("drawing.linkCel", json!({"from": 3, "frame": 16})),
        ("drawing.linkCel", json!({"from": -1, "frame": 16})),
        ("drawing.linkCel", json!({"from": "a", "frame": 16})),
        ("drawing.linkCel", json!({"from": 1.5, "frame": 16})),
        ("drawing.linkCel", json!({"from": 0, "frame": -4})),
        ("drawing.linkCel", json!({"from": 0, "frame": 9_000_000_000_000i64})),
        ("drawing.linkCel", json!({"from": 0, "frame": 4})),
        ("drawing.linkCel", json!({"from": i64::MIN, "frame": 20})),
        ("drawing.makeIndependent", json!({"frame": 4})),
        ("drawing.makeIndependent", json!({"frame": -3})),
    ] {
        assert!(s.execute(id, params.clone()).is_err(), "{id} {params}");
        assert_eq!(snapshot(&s, clip), before, "{id} {params} must change nothing");
    }
    let track = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(id, _)| id).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let locked = snapshot(&s, clip);
    for (id, params) in [("drawing.linkCel", json!({"from": 0, "frame": 16})), ("drawing.makeIndependent", json!({"frame": 8}))] {
        let e = s.execute(id, params).unwrap_err();
        assert!(e.to_string().contains("locked"), "{id}: {e}");
        assert_eq!(snapshot(&s, clip), locked);
    }
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": false})).unwrap();
    // a clip with no drawing under the playhead is refused with the usual message
    assert!(Session::default().execute("drawing.linkCel", json!({"from": 0, "frame": 8})).is_err());
}

#[test]
fn an_edit_that_leaves_peers_different_is_refused_and_rolled_back() {
    let (mut s, clip) = blink();
    let before = snapshot(&s, clip);
    let item = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(_, it)| it.item).unwrap();
    // a path that changes one linked cel without its peers (what `edit_strokes` exists to prevent)
    let e = crate::drawing::edit_drawing(&mut s, "test", "Bad Edit", item, |d| {
        d.cels[0].strokes_mut().clear();
        Ok(())
    })
    .unwrap_err();
    assert!(e.to_string().contains("does not show its group"), "{e}");
    assert_eq!(snapshot(&s, clip), before);
}

#[test]
fn inspect_reports_links_and_how_many_exposures_share() {
    let (s, clip) = blink();
    let mut s = s;
    let r = s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap();
    let ex = r["exposures"].as_array().unwrap();
    assert_eq!(ex.iter().map(|e| e["linked"].as_u64()).collect::<Vec<_>>(), vec![Some(3), None, Some(3), Some(3)]);
    assert!(ex[0]["link"].is_u64() && ex[1].get("link").is_none() && ex[0]["link"] == ex[3]["link"]);
    assert!(r["cels"][1].get("link").is_none(), "an ordinary cel is reported as it always was");
    assert!(r["cels"][2]["link"].is_u64());
}

#[test]
fn linked_exposures_share_one_cache_entry_and_an_edit_misses_for_all() {
    use waltcut_render::drawing_cache::CelRasterCache;
    let (mut s, clip) = session();
    // enough samples for the cache to take the cel
    let pts: Vec<Value> = (0..2500).map(|i| json!([10.0 + (i % 140) as f64, 10.0 + (i / 40) as f64 % 70.0, 1.0])).collect();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Pen", "size": 3, "color": "#102030", "points": pts}})).unwrap();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 5})).unwrap();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 9})).unwrap();
    s.execute("drawing.newCel", json!({"frame": 13, "copy": true})).unwrap();
    let m = waltcut_geom::Affine::IDENTITY;
    let cache = CelRasterCache::new(64 << 20, 16, 2048);
    let d = drawing(&s, clip);
    let first = cache.rasterise(&d.cels[0].strokes, &m, 160, 90);
    let (b, c) = (cache.rasterise(&d.cels[1].strokes, &m, 160, 90), cache.rasterise(&d.cels[2].strokes, &m, 160, 90));
    let stats = cache.stats();
    assert_eq!((stats.misses, stats.hits, stats.entries), (1, 2, 1), "three exposures, one rasterisation");
    assert!(first.px == b.px && b.px == c.px);
    // a duplicate holds the same strokes until it is edited (copy on write): the same raster; once
    // edited it is its own cel with its own entry
    cache.rasterise(&d.cels[3].strokes, &m, 160, 90);
    assert_eq!(cache.stats().entries, 1);
    s.execute("drawing.addStroke", json!({"frame": 13, "stroke": dash("#00ff00", 15.0)})).unwrap();
    cache.rasterise(&drawing(&s, clip).cels[3].strokes, &m, 160, 90);
    assert_eq!(cache.stats().entries, 2);
    let d = drawing(&s, clip);
    assert_eq!(d.cels.iter().map(|c| c.strokes.len()).collect::<Vec<_>>(), vec![1, 1, 1, 2], "editing the duplicate left the group alone");
    // an edit of one exposure gives the whole group a new allocation: none of them hits the old entry
    s.execute("drawing.addStroke", json!({"frame": 9, "stroke": dash("#ff0000", 45.0)})).unwrap();
    let e = drawing(&s, clip);
    let before = cache.stats();
    let new_b = cache.rasterise(&e.cels[1].strokes, &m, 160, 90);
    assert_ne!(new_b.px, first.px);
    let after = cache.stats();
    assert_eq!(after.misses + after.extended, before.misses + before.extended + 1, "the edited group is rasterised again");
    let new_c = cache.rasterise(&e.cels[2].strokes, &m, 160, 90);
    assert_eq!(cache.stats().hits, after.hits + 1, "and then its peers share that entry");
    assert_eq!(new_b.px, new_c.px);
}
