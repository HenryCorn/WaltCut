//! Guide clips (#65): shown when asked for (the Program preview), never in a render nobody asked
//! guides for, on the CPU path, the frame plan the GPU gets, and inside nested sequences.

use super::*;
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{Generator, MediaSource};
use waltcut_project::{ClipId, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use waltcut_time::{FrameRate, TICKS_PER_SECOND, TimeRange};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

struct Rig {
    p: Project,
    map: SourceMap,
}

impl Rig {
    fn new() -> Rig {
        Rig { p: Project::new("guides"), map: SourceMap::default() }
    }

    fn matte(&mut self, color: [f32; 4]) -> ItemId {
        let g = GeneratorSource::new(Generator::ColorMatte { color }, 160, 90, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND));
        let info = g.info().clone();
        let id = self.p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(g.generator.clone()),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                identity: None,
            }),
            None,
        );
        self.map.0.insert(id, Arc::new(g) as SharedSource);
        id
    }

    fn seq(&mut self, name: &str) -> ItemId {
        self.p.new_sequence(name, SequenceSettings { width: 160, height: 90, frame_rate: FrameRate::FPS_24, ..Default::default() }, 2, 1, None)
    }

    /// 24 frames of `item` on video track `track` of `seq`.
    fn put(&mut self, seq: ItemId, track: usize, item: ItemId) -> ClipId {
        let r = FrameRate::FPS_24;
        let mut ti = self.p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
        ti.scale_to_frame = true;
        let id = ti.id;
        self.p.sequence_mut(seq).unwrap().video_tracks[track].items.push(ti);
        id
    }

    fn set_guide(&mut self, seq: ItemId, clip: ClipId, guide: bool) {
        self.p.sequence_mut(seq).unwrap().find_item_mut(clip).unwrap().1.guide = guide;
    }

    fn centre(&self, seq: ItemId, guides: bool) -> [f32; 4] {
        let opts = RenderOptions { guides, ..Default::default() };
        let cpu = render_sequence(&self.p, seq, Tick(1000), opts, &self.map).get(80, 45);
        let planned = plan::execute_cpu(&plan::plan_frame(&self.p, seq, Tick(1000), opts, &self.map)).get(80, 45);
        assert!(close(cpu, planned), "the plan agrees with the CPU render: {cpu:?} vs {planned:?}");
        cpu
    }
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.02)
}

#[test]
fn a_guide_is_in_the_preview_and_never_in_a_default_render() {
    let mut r = Rig::new();
    let (green, red) = (r.matte(GREEN), r.matte(RED));
    let seq = r.seq("s");
    r.put(seq, 0, green);
    let reference = r.put(seq, 1, red);
    assert!(close(r.centre(seq, false), RED) && close(r.centre(seq, true), RED), "an ordinary clip is always drawn");
    r.set_guide(seq, reference, true);
    assert!(close(r.centre(seq, true), RED), "the preview shows the guide");
    assert!(close(r.centre(seq, false), GREEN), "a delivery render shows what is under it");
    assert!(!RenderOptions::default().guides, "guides are off unless a caller asks for them");
    // taking the flag off brings it back everywhere
    r.set_guide(seq, reference, false);
    assert!(close(r.centre(seq, false), RED));
}

#[test]
fn a_guide_alone_leaves_the_frame_empty_in_a_delivery_render() {
    let mut r = Rig::new();
    let red = r.matte(RED);
    let seq = r.seq("s");
    let reference = r.put(seq, 0, red);
    r.set_guide(seq, reference, true);
    assert!(r.centre(seq, false)[3] < 0.01, "transparent, not the guide");
    assert!(close(r.centre(seq, true), RED));
}

#[test]
fn a_guide_inside_a_nested_sequence_does_not_leak_either() {
    let mut r = Rig::new();
    let (green, red) = (r.matte(GREEN), r.matte(RED));
    let inner = r.seq("inner");
    r.put(inner, 0, green);
    let reference = r.put(inner, 1, red);
    r.set_guide(inner, reference, true);
    let outer = r.seq("outer");
    r.put(outer, 0, inner);
    assert!(close(r.centre(outer, false), GREEN), "the nest shows its picture without the guide");
    assert!(close(r.centre(outer, true), RED), "the preview shows it through the nest");
    // the nested guide as a whole nest clip of the outer sequence
    let nest = r.put(outer, 1, inner);
    r.set_guide(outer, nest, true);
    assert!(close(r.centre(outer, false), GREEN));
}
