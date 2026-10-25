//! The stored result of a fill (#68): a run-length mask over a rectangle of the drawing's canvas.
//!
//! A fill is one operation in a cel's paint order (a [`Stroke`](crate::drawing::Stroke) with
//! `brush: Fill`). Its result is kept, not recomputed: see `docs/animation.md` (Fill a region) for
//! the rules and `docs/project-files.md` for the file shape. This module is the data and its
//! bounds; the flood fill that makes one is `waltcut_render::drawing_fill`.

use serde::{Deserialize, Serialize};

/// The largest canvas, in pixels, that can be filled or that a fill is accepted for (4096 × 2160).
pub const MAX_FILL_PIXELS: u64 = 8_847_360;
/// The most runs one fill may hold.
pub const MAX_FILL_RUNS: usize = 65_536;
/// The most fill runs one cel may hold, over all its fills.
pub const MAX_CEL_FILL_RUNS: usize = 262_144;
/// The most fill runs one drawing may hold, over all its cels.
pub const MAX_DRAWING_FILL_RUNS: usize = 2_097_152;

/// The largest gap-closing radius, in canvas pixels (#69): openings up to twice this are closed.
pub const MAX_FILL_GAP: u8 = 8;
/// The most extra edge pixels a fill may expand by (#69), on top of the one-pixel shell.
pub const MAX_FILL_GROW: u8 = 8;

/// A cell outside the region.
pub const CLASS_OUTSIDE: u8 = 0;
/// A cell of the region: painted over what is there.
pub const CLASS_FILLED: u8 = 1;
/// A cell of the one-pixel shell around the region: painted behind what is there.
pub const CLASS_EDGE: u8 = 2;

/// The region a fill painted: the `seed` and `tolerance` it was made with and the result.
///
/// `rect` is `[x, y, w, h]` in canvas pixels, inside the canvas. `runs` is a run-length mask over
/// the rectangle's cells, row by row: `[class, length]` pairs (see [`CLASS_OUTSIDE`],
/// [`CLASS_FILLED`], [`CLASS_EDGE`]). Lengths are at least 1, they add up to `w × h` and two
/// neighbouring runs never share a class, so one mask has exactly one encoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    pub seed: [u32; 2],
    pub tolerance: u8,
    pub rect: [u32; 4],
    pub runs: Vec<[u32; 2]>,
    /// The gap-closing radius the region was found with (0 when none; written only when set).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gap: u8,
    /// The edge expansion beyond the one-pixel shell (0 when none; written only when set).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub grow: u8,
    /// The drawing clip the region was found in, when it was not the cel itself (#69). Provenance
    /// for redoing the fill: the mask alone is drawn, so a reference that is gone changes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<u64>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

impl Fill {
    /// Encode `plane` (one class byte per cell of `rect`, row-major) as a fill.
    ///
    /// Fails, with nothing stored, when the plane does not match the rectangle or needs more than
    /// [`MAX_FILL_RUNS`] runs (the error says how many it needed).
    pub fn encode(seed: [u32; 2], tolerance: u8, rect: [u32; 4], plane: &[u8]) -> Result<Fill, String> {
        let cells = u64::from(rect[2]) * u64::from(rect[3]);
        if cells == 0 || cells != plane.len() as u64 {
            return Err("the fill mask does not match its rectangle".into());
        }
        let mut runs: Vec<[u32; 2]> = Vec::new();
        let mut needed = 0usize;
        let mut it = plane.iter().copied();
        let Some(mut class) = it.next() else { return Err("the fill is empty".into()) };
        let mut len = 1u32;
        for c in it {
            if c == class {
                len = len.checked_add(1).ok_or("the fill is too large")?;
                continue;
            }
            needed += 1;
            if runs.len() < MAX_FILL_RUNS {
                runs.push([u32::from(class), len]);
            }
            class = c;
            len = 1;
        }
        needed += 1;
        if runs.len() < MAX_FILL_RUNS {
            runs.push([u32::from(class), len]);
        }
        if needed > MAX_FILL_RUNS {
            return Err(format!("that region is too ragged to store: it needs {needed} runs and a fill holds at most {MAX_FILL_RUNS}"));
        }
        Ok(Fill { seed, tolerance, rect, runs, gap: 0, grow: 0, reference: None })
    }

    /// This fill, recorded as found with the options of #69.
    pub fn with_options(mut self, gap: u8, grow: u8, reference: Option<u64>) -> Fill {
        self.gap = gap;
        self.grow = grow;
        self.reference = reference;
        self
    }

    /// The reason this is not a usable fill for a canvas of `canvas_w × canvas_h`, if it is not.
    pub fn validate(&self, canvas_w: u32, canvas_h: u32) -> Result<(), String> {
        if u64::from(canvas_w) * u64::from(canvas_h) > MAX_FILL_PIXELS {
            return Err(format!("a canvas of {canvas_w} × {canvas_h} is too large to hold a fill"));
        }
        if self.gap > MAX_FILL_GAP || self.grow > MAX_FILL_GROW || self.reference == Some(0) {
            return Err(format!("a fill's gap and grow are at most {MAX_FILL_GAP} and {MAX_FILL_GROW}, and its reference a clip id"));
        }
        let [x, y, w, h] = self.rect;
        if w == 0 || h == 0 {
            return Err("a fill's rectangle is empty".into());
        }
        if u64::from(x) + u64::from(w) > u64::from(canvas_w) || u64::from(y) + u64::from(h) > u64::from(canvas_h) {
            return Err("a fill's rectangle is outside the canvas".into());
        }
        if self.seed[0] < x || self.seed[0] - x >= w || self.seed[1] < y || self.seed[1] - y >= h {
            return Err("a fill's seed is outside its rectangle".into());
        }
        if self.runs.is_empty() || self.runs.len() > MAX_FILL_RUNS {
            return Err(format!("a fill holds 1 to {MAX_FILL_RUNS} runs, not {}", self.runs.len()));
        }
        let cells = u64::from(w) * u64::from(h);
        let seed_cell = u64::from(self.seed[1] - y) * u64::from(w) + u64::from(self.seed[0] - x);
        let (mut at, mut previous, mut seed_class) = (0u64, None, None);
        for &[class, len] in &self.runs {
            if class > u32::from(CLASS_EDGE) || len == 0 || previous == Some(class) {
                return Err("a fill's runs are not a valid encoding".into());
            }
            let next = at.checked_add(u64::from(len)).filter(|n| *n <= cells).ok_or("a fill's runs are longer than its rectangle")?;
            if (at..next).contains(&seed_cell) {
                seed_class = Some(class);
            }
            at = next;
            previous = Some(class);
        }
        if at != cells {
            return Err("a fill's runs do not cover its rectangle".into());
        }
        if seed_class != Some(u32::from(CLASS_FILLED)) {
            return Err("a fill's seed is not inside its region".into());
        }
        Ok(())
    }

    /// The mask as one class byte per cell of the rectangle, row-major. Fails on a fill that does
    /// not [`validate`](Fill::validate)-shape (runs that do not add up), never allocates more than
    /// [`MAX_FILL_PIXELS`] cells.
    pub fn decode(&self) -> Result<Vec<u8>, String> {
        let cells = u64::from(self.rect[2]) * u64::from(self.rect[3]);
        if cells == 0 || cells > MAX_FILL_PIXELS {
            return Err("a fill's rectangle is empty or too large".into());
        }
        let mut plane = vec![0u8; cells as usize];
        let mut at = 0usize;
        for &[class, len] in &self.runs {
            let end = at.checked_add(len as usize).filter(|e| *e <= plane.len()).ok_or("a fill's runs are longer than its rectangle")?;
            let class = u8::try_from(class).ok().filter(|c| *c <= CLASS_EDGE).ok_or("a fill has an unknown class")?;
            if class != CLASS_OUTSIDE
                && let Some(cells) = plane.get_mut(at..end)
            {
                cells.fill(class);
            }
            at = end;
        }
        if at != plane.len() {
            return Err("a fill's runs do not cover its rectangle".into());
        }
        Ok(plane)
    }

    /// The cells of the region itself (class 1).
    pub fn filled_pixels(&self) -> u64 {
        self.runs.iter().filter(|r| r[0] == u32::from(CLASS_FILLED)).map(|r| u64::from(r[1])).sum()
    }

    /// `(min_x, min_y, max_x, max_y)` of the rectangle, canvas pixels.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let [x, y, w, h] = self.rect;
        (x as f32, y as f32, x as f32 + w as f32, y as f32 + h as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(rows: &[&str]) -> (Vec<u8>, u32, u32) {
        let cells = rows.iter().flat_map(|r| r.chars().map(|c| c.to_digit(10).unwrap_or(0) as u8)).collect();
        (cells, rows[0].len() as u32, rows.len() as u32)
    }

    #[test]
    fn a_mask_survives_encoding_and_decoding_exactly() {
        let (cells, w, h) = plane(&["0220", "2112", "2112", "0220"]);
        let f = Fill::encode([4, 6], 32, [3, 5, w, h], &cells).unwrap();
        f.validate(20, 20).unwrap();
        assert_eq!(f.decode().unwrap(), cells);
        assert_eq!(f.filled_pixels(), 4);
        assert_eq!(f.bounds(), (3.0, 5.0, 7.0, 9.0));
        // one mask, one encoding: neighbouring runs never share a class
        assert!(f.runs.windows(2).all(|w| w[0][0] != w[1][0]));
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Fill>(&json).unwrap(), f);
    }

    #[test]
    fn options_are_written_only_when_set_and_survive_json() {
        let plain = Fill::encode([1, 1], 0, [0, 0, 4, 4], &[1u8; 16]).unwrap();
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("gap") && !json.contains("grow") && !json.contains("reference"), "{json}");
        let tuned = plain.clone().with_options(3, 2, Some(41));
        tuned.validate(4, 4).unwrap();
        let json = serde_json::to_string(&tuned).unwrap();
        assert!(json.contains("\"gap\":3") && json.contains("\"grow\":2") && json.contains("\"reference\":41"), "{json}");
        assert_eq!(serde_json::from_str::<Fill>(&json).unwrap(), tuned);
        // a file without the keys is the plain fill
        assert_eq!(serde_json::from_str::<Fill>(&serde_json::to_string(&plain).unwrap()).unwrap(), plain);
        assert!(serde_json::from_str::<Fill>(r#"{"seed":[0,0],"tolerance":0,"rect":[0,0,1,1],"runs":[[1,1]],"gap":300}"#).is_err());
    }

    #[test]
    fn a_whole_canvas_is_one_run() {
        let f = Fill::encode([0, 0], 0, [0, 0, 1920, 1080], &vec![1u8; 1920 * 1080]).unwrap();
        assert_eq!(f.runs, vec![[1, 1920 * 1080]]);
        f.validate(1920, 1080).unwrap();
    }

    #[test]
    fn a_region_that_needs_too_many_runs_is_refused_with_the_count() {
        // a checkerboard row of 70 000 cells: one run per cell
        let cells: Vec<u8> = (0..70_000u32).map(|i| (i % 2) as u8).collect();
        let e = Fill::encode([1, 0], 0, [0, 0, 70_000, 1], &cells).unwrap_err();
        assert!(e.contains("70000") && e.contains("65536"), "{e}");
    }

    #[test]
    fn hostile_fills_do_not_validate_or_decode() {
        let good = Fill::encode([1, 1], 0, [0, 0, 4, 4], &[1u8; 16]).unwrap();
        good.validate(4, 4).unwrap();
        let mut cases: Vec<(&str, Fill)> = Vec::new();
        let with = |f: &dyn Fn(&mut Fill)| {
            let mut x = good.clone();
            f(&mut x);
            x
        };
        cases.push(("outside the canvas", with(&|f| f.rect = [2, 2, 4, 4])));
        cases.push(("overflowing rect", with(&|f| f.rect = [u32::MAX, 0, 4, 4])));
        cases.push(("empty rect", with(&|f| f.rect = [0, 0, 0, 4])));
        cases.push(("seed outside", with(&|f| f.seed = [9, 9])));
        cases.push(("no runs", with(&|f| f.runs.clear())));
        cases.push(("zero length", with(&|f| f.runs = vec![[1, 0], [0, 16]])));
        cases.push(("same class twice", with(&|f| f.runs = vec![[1, 8], [1, 8]])));
        cases.push(("unknown class", with(&|f| f.runs = vec![[7, 16]])));
        cases.push(("too short", with(&|f| f.runs = vec![[1, 15]])));
        cases.push(("too long", with(&|f| f.runs = vec![[1, 17]])));
        cases.push(("length overflow", with(&|f| f.runs = vec![[1, u32::MAX], [0, u32::MAX]])));
        cases.push(("gap too wide", with(&|f| f.gap = MAX_FILL_GAP + 1)));
        cases.push(("grow too wide", with(&|f| f.grow = MAX_FILL_GROW + 1)));
        cases.push(("reference zero", with(&|f| f.reference = Some(0))));
        cases.push(("seed not filled", with(&|f| f.runs = vec![[0, 16]])));
        for (name, f) in cases {
            assert!(f.validate(4, 4).is_err(), "{name} must not validate");
            // decoding never panics and never allocates past the cap, whatever the shape
            let _ = f.decode();
        }
        assert!(good.validate(4096, 4096).is_err(), "a canvas over the pixel cap holds no fill");
        let huge = Fill { seed: [0, 0], tolerance: 0, rect: [0, 0, u32::MAX, u32::MAX], runs: vec![[1, 1]], gap: 0, grow: 0, reference: None };
        assert!(huge.decode().is_err());
        assert!(serde_json::from_str::<Fill>(r#"{"seed":[0,0],"tolerance":0,"rect":[0,0,1,1],"runs":[[1,1]],"extra":1}"#).is_err());
        assert!(serde_json::from_str::<Fill>(r#"{"seed":[0,0],"tolerance":300,"rect":[0,0,1,1],"runs":[[1,1]]}"#).is_err());
    }
}
