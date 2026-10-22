//! Extension filters for native file dialogs.
//!
//! Every filtered file dialog (import, relink, open file/preset, open/save project) builds
//! its filter with [`extensions`]. macOS dialogs ignore case and receive the plain list of
//! literal extensions.

pub fn extensions(exts: &[&str]) -> Vec<String> {
    let extensions: Vec<_> = exts.iter().map(|ext| (*ext).to_owned()).collect();
    extensions
}

#[cfg(test)]
mod tests {
    use super::extensions;

    #[test]
    fn save_filters_keep_the_literal_default_extension() {
        assert_eq!(extensions(&["mp4", "mov"]).first().map(String::as_str), Some("mp4"));
        assert!(extensions(&[]).is_empty());
        assert_eq!(extensions(&["*", "", "123"]), ["*", "", "123"]);
    }

    #[test]
    fn native_filters_receive_only_literal_extensions() {
        assert_eq!(extensions(&["mp4", "mov", "fcproj"]), ["mp4", "mov", "fcproj"]);
    }
}
