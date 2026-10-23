//! Transactional disk output for one export. The original destination survives failed/cancelled
//! encoding. PNG sequences require unused names; only files installed by this job can be removed.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) const MAX_FRAMES: u64 = 1_000_000;

pub(crate) struct ExportOutput {
    destination: PathBuf,
    directory: PathBuf,
    staged: PathBuf,
    frames: Option<u64>,
    overwrite: bool,
    installed: u64,
}

fn numbered(path: &Path, frame: u64, count: u64) -> io::Result<PathBuf> {
    let stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("invalid output filename"))?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| s.eq_ignore_ascii_case("png"))
        .ok_or_else(|| io::Error::other("PNG sequence needs a .png extension"))?;
    let digits = count.saturating_sub(1).max(1).to_string().len().max(3);
    Ok(path.with_file_name(format!("{stem}{frame:0digits$}.{extension}")))
}

fn unused(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists; choose another destination or explicitly overwrite a movie", path.display()),
        )),
    }
}

impl ExportOutput {
    pub(crate) fn new(destination: &Path, frames: Option<u64>, overwrite: bool) -> io::Result<Self> {
        if frames.is_some_and(|n| n == 0 || n > MAX_FRAMES) {
            return Err(io::Error::other("PNG sequence must contain 1..1000000 frames"));
        }
        if frames.is_some() && overwrite {
            return Err(io::Error::other("PNG sequence overwrite is not supported; choose an unused filename base"));
        }
        if let Some(count) = frames {
            unused(&numbered(destination, 0, count)?)?;
        } else if !overwrite {
            unused(destination)?;
        } else if destination.symlink_metadata().is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
            return Err(io::Error::other("overwrite destination must be a regular file, not a symlink or directory"));
        }
        let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let name = destination.file_name().ok_or_else(|| io::Error::other("output needs a filename"))?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..32 {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!(".waltcut-export-{}-{serial}", std::process::id()));
            match std::fs::create_dir(&directory) {
                Ok(()) => {
                    return Ok(Self { destination: destination.to_path_buf(), staged: directory.join(name), directory, frames, overwrite, installed: 0 });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("cannot reserve an export staging directory"))
    }

    pub(crate) fn staged_path(&self) -> &Path {
        &self.staged
    }

    /// Potentially many filesystem probes run inside the cancellable worker, not the UI thread.
    pub(crate) fn prepare(&self, cancel: &AtomicBool) -> io::Result<()> {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if let Some(count) = self.frames {
            for frame in 0..count {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                unused(&numbered(&self.destination, frame, count)?)?;
            }
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self, cancel: &AtomicBool) -> io::Result<()> {
        let check = || {
            if cancel.load(Ordering::Relaxed) { Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")) } else { Ok(()) }
        };
        check()?;
        if let Some(count) = self.frames {
            for frame in 0..count {
                check()?;
                let destination = numbered(&self.destination, frame, count)?;
                let source = numbered(&self.staged, frame, count)?;
                // Atomic no-clobber install: an intervening writer cannot lose its file.
                std::fs::hard_link(&source, &destination)?;
                self.installed = frame.saturating_add(1);
            }
        } else if self.overwrite {
            // On macOS rename atomically replaces the destination only after encoding succeeds.
            std::fs::rename(&self.staged, &self.destination)?;
        } else {
            std::fs::hard_link(&self.staged, &self.destination)?;
        }
        self.installed = 0;
        Ok(())
    }

    pub(crate) fn cleanup(&mut self) -> io::Result<()> {
        let mut failure = None;
        for frame in 0..self.installed {
            let count = self.frames.ok_or_else(|| io::Error::other("partial image export has no frame count"))?;
            let source = numbered(&self.staged, frame, count)?;
            let destination = numbered(&self.destination, frame, count)?;
            let remove_owned = || -> io::Result<()> {
                let metadata = match destination.symlink_metadata() {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    result => result?,
                };
                if !metadata.file_type().is_symlink() && same_file::is_same_file(&source, &destination)? {
                    std::fs::remove_file(&destination)?;
                }
                Ok(())
            };
            if let Err(e) = remove_owned()
                && failure.is_none()
            {
                failure = Some(io::Error::other(format!(
                    "could not remove partial export {}: {e}; staging retained at {}",
                    destination.display(),
                    self.directory.display()
                )));
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        self.installed = 0;
        match std::fs::remove_dir_all(&self.directory) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io::Error::other(format!("could not remove export staging directory {}: {e}", self.directory.display()))),
        }
    }
}

impl Drop for ExportOutput {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            let _ = writeln!(io::stderr().lock(), "{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path = PathBuf::from("target").join("export-output-tests").join(format!("{}-{name}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn cancelled_movie_keeps_existing_destination_and_removes_only_staging() {
        let dir = Scratch::new("cancel");
        let path = dir.0.join("movie.mov");
        std::fs::write(&path, b"original").unwrap();
        let stage;
        {
            let mut output = ExportOutput::new(&path, None, true).unwrap();
            stage = output.directory.clone();
            std::fs::write(output.staged_path(), b"partial").unwrap();
            assert_eq!(output.commit(&AtomicBool::new(true)).unwrap_err().kind(), io::ErrorKind::Interrupted);
        }
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert!(!stage.exists());
    }

    #[test]
    fn incomplete_sequence_rolls_back_only_its_own_installed_frames() {
        let dir = Scratch::new("sequence");
        let path = dir.0.join("frame.png");
        let unrelated = dir.0.join("frame002.png");
        {
            let mut output = ExportOutput::new(&path, Some(3), false).unwrap();
            std::fs::write(numbered(output.staged_path(), 0, 3).unwrap(), b"frame 0").unwrap();
            std::fs::write(&unrelated, b"another writer").unwrap();
            assert!(output.commit(&AtomicBool::new(false)).is_err());
        }
        assert!(!dir.0.join("frame000.png").exists());
        assert!(!dir.0.join("frame001.png").exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"another writer");
    }

    #[test]
    fn existing_output_and_hostile_counts_are_rejected_before_staging() {
        let dir = Scratch::new("hostile");
        let path = dir.0.join("movie.mp4");
        std::fs::write(&path, b"original").unwrap();
        assert!(ExportOutput::new(&path, None, false).is_err());
        for count in [0, MAX_FRAMES + 1, u64::MAX] {
            assert!(ExportOutput::new(&path, Some(count), false).is_err());
        }
        assert!(ExportOutput::new(&path, Some(1), true).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn successful_movie_and_sequence_install_exact_files() {
        let dir = Scratch::new("success");
        for frames in [None, Some(2)] {
            let path = dir.0.join(if frames.is_some() { "frame.png" } else { "movie.mp4" });
            let mut output = ExportOutput::new(&path, frames, false).unwrap();
            if let Some(count) = frames {
                for frame in 0..count {
                    std::fs::write(numbered(output.staged_path(), frame, count).unwrap(), [frame as u8]).unwrap();
                }
            } else {
                std::fs::write(output.staged_path(), b"movie").unwrap();
            }
            output.commit(&AtomicBool::new(false)).unwrap();
            drop(output);
            if frames.is_some() {
                assert_eq!(std::fs::read(dir.0.join("frame000.png")).unwrap(), [0]);
                assert_eq!(std::fs::read(dir.0.join("frame001.png")).unwrap(), [1]);
            } else {
                assert_eq!(std::fs::read(path).unwrap(), b"movie");
            }
        }
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 3);
    }

    #[test]
    fn destination_preflight_is_cancellable_and_detects_later_frame_conflicts() {
        let dir = Scratch::new("preflight");
        let output = ExportOutput::new(&dir.0.join("seq.png"), Some(3), false).unwrap();
        std::fs::write(dir.0.join("seq002.png"), b"existing").unwrap();
        assert_eq!(output.prepare(&AtomicBool::new(false)).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(output.prepare(&AtomicBool::new(true)).unwrap_err().kind(), io::ErrorKind::Interrupted);
        drop(output);
        assert_eq!(std::fs::read(dir.0.join("seq002.png")).unwrap(), b"existing");
    }

    #[test]
    fn numbered_output_preserves_the_requested_png_extension_case() {
        let dir = Scratch::new("upper-case");
        let destination = dir.0.join("Frame.PNG");
        let mut output = ExportOutput::new(&destination, Some(2), false).unwrap();
        std::fs::write(output.staged_path().with_file_name("Frame000.PNG"), b"frame 0").unwrap();
        std::fs::write(output.staged_path().with_file_name("Frame001.PNG"), b"frame 1").unwrap();
        output.commit(&AtomicBool::new(false)).unwrap();
        output.cleanup().unwrap();
        assert_eq!(std::fs::read(dir.0.join("Frame000.PNG")).unwrap(), b"frame 0");
        assert_eq!(std::fs::read(dir.0.join("Frame001.PNG")).unwrap(), b"frame 1");
    }

    #[test]
    fn cleanup_errors_identify_the_staging_path_and_can_be_retried() {
        let dir = Scratch::new("cleanup-error");
        let mut output = ExportOutput::new(&dir.0.join("movie.mov"), None, false).unwrap();
        std::fs::remove_dir(&output.directory).unwrap();
        std::fs::write(&output.directory, b"not a directory").unwrap();
        let error = output.cleanup().unwrap_err().to_string();
        assert!(error.contains(output.directory.to_str().unwrap()), "{error}");
        std::fs::remove_file(&output.directory).unwrap();
        output.cleanup().unwrap();
    }

    #[test]
    fn a_racing_movie_writer_is_not_overwritten_without_permission() {
        let dir = Scratch::new("movie-race");
        let destination = dir.0.join("movie.mov");
        {
            let mut output = ExportOutput::new(&destination, None, false).unwrap();
            std::fs::write(output.staged_path(), b"our movie").unwrap();
            std::fs::write(&destination, b"another writer").unwrap();
            assert_eq!(output.commit(&AtomicBool::new(false)).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        }
        assert_eq!(std::fs::read(destination).unwrap(), b"another writer");
    }

    #[test]
    fn sequence_rollback_does_not_remove_a_replacement_file() {
        let dir = Scratch::new("replacement");
        let destination = dir.0.join("seq000.png");
        {
            let mut output = ExportOutput::new(&dir.0.join("seq.png"), Some(2), false).unwrap();
            std::fs::write(numbered(output.staged_path(), 0, 2).unwrap(), b"owned").unwrap();
            assert!(output.commit(&AtomicBool::new(false)).is_err(), "second staged frame is missing");
            std::fs::remove_file(&destination).unwrap();
            std::fs::write(&destination, b"replacement owned by another writer").unwrap();
        }
        assert_eq!(std::fs::read(destination).unwrap(), b"replacement owned by another writer");
    }

    #[test]
    fn a_panicking_encoder_preserves_the_original_and_cleans_its_staging() {
        let dir = Scratch::new("panic");
        let destination = dir.0.join("movie.mov");
        std::fs::write(&destination, b"original").unwrap();
        let mut stage = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let output = ExportOutput::new(&destination, None, true).unwrap();
            stage = Some(output.directory.clone());
            std::fs::write(output.staged_path(), b"unfinished").unwrap();
            panic!("encoder failed");
        }));
        assert!(outcome.is_err());
        assert!(!stage.unwrap().exists());
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn explicit_movie_overwrite_installs_only_a_finished_file() {
        let dir = Scratch::new("overwrite");
        let destination = dir.0.join("movie.mov");
        std::fs::write(&destination, b"original").unwrap();
        let mut output = ExportOutput::new(&destination, None, true).unwrap();
        std::fs::write(output.staged_path(), b"complete replacement").unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        output.commit(&AtomicBool::new(false)).unwrap();
        drop(output);
        assert_eq!(std::fs::read(destination).unwrap(), b"complete replacement");
    }
}
