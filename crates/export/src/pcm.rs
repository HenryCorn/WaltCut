//! Little-endian float PCM for the retained ProRes MOV soundtrack and PNG sequence naming.

pub(crate) fn encode(interleaved: &[f32], out: &mut Vec<u8>) {
    for &sample in interleaved {
        let sample = if sample.is_nan() { 0.0 } else { sample.clamp(f32::MIN, f32::MAX) };
        out.extend_from_slice(&sample.to_le_bytes());
    }
}

/// File name of frame `index` of an image sequence of `count` frames exported to `path`:
/// the file name's stem followed directly by the zero-padded frame number (at least three
/// digits, more when the count needs them), starting at 000, like Premiere's numbered stills:
/// `Sequence 01.png` → `Sequence 01000.png`, `Sequence 01001.png`, …
pub fn image_sequence_path(path: &str, index: u64, count: u64) -> String {
    let p = std::path::Path::new(path);
    let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let digits = count.saturating_sub(1).max(1).to_string().len().max(3);
    let name = format!("{stem}{index:0digits$}.{ext}");
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.join(name).to_string_lossy().to_string(),
        None => name,
    }
}
