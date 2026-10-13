//! Resolving image sources and reading their natural size (so they can fit the reading column).

use std::path::{Path, PathBuf};

use crate::markdown::links::percent_decode;

#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    Local { path: PathBuf, width: f32, height: f32 },
    /// Never fetched; shown as a placeholder with the address.
    Remote(String),
    /// `path` as written, and why it cannot be shown.
    Unavailable { path: String, reason: &'static str },
    /// An image of a remote document being read from its host (R2a section 8.3).
    Pending,
}

const RASTER: [&str; 4] = ["png", "jpg", "jpeg", "gif"];

/// Where a local-looking `src` points (relative to the document's directory), or the image to show without any I/O.
pub fn locate(src: &str, base_dir: Option<&Path>) -> Result<PathBuf, Image> {
    if ["http://", "https://", "//"].iter().any(|p| src.starts_with(p)) {
        return Err(Image::Remote(src.to_string()));
    }
    let decoded = percent_decode(src);
    let local = Path::new(&decoded);
    if local.is_absolute() {
        Ok(local.to_path_buf())
    } else if let Some(dir) = base_dir {
        Ok(dir.join(local))
    } else {
        Err(Image::Unavailable { path: src.to_string(), reason: crate::i18n::text("找不到图片", "Image not found") })
    }
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn supported(path: &Path) -> bool {
    let ext = ext_of(path);
    ext == "svg" || RASTER.contains(&ext.as_str())
}

/// Resolves `src` relative to the Markdown file's directory and reads its size. Blocking (file I/O).
pub fn resolve(src: &str, base_dir: Option<&Path>) -> Image {
    let t = crate::i18n::text;
    let path = match locate(src, base_dir) {
        Ok(p) => p,
        Err(image) => return image,
    };
    let unavailable = |reason| Image::Unavailable { path: src.to_string(), reason };
    if !path.is_file() {
        return unavailable(t("找不到图片", "Image not found"));
    }
    if !supported(&path) {
        return unavailable(t("不支持的图片格式", "Unsupported image format"));
    }
    let size = if ext_of(&path) == "svg" { std::fs::read(&path).ok().and_then(|d| svg_size(&d)) } else { imagesize::size(&path).ok().map(|s| (s.width as f32, s.height as f32)) };
    match size.filter(|&(w, h)| w > 0.0 && h > 0.0) {
        Some((width, height)) => Image::Local { path, width, height },
        None => unavailable(t("无法读取图片", "Could not read image")),
    }
}

/// A remote image saved at `path` on this Mac (the cache): its size from `bytes`.
pub fn from_bytes(path: PathBuf, bytes: &[u8]) -> Image {
    let size = if ext_of(&path) == "svg" { svg_size(bytes) } else { imagesize::blob_size(bytes).ok().map(|s| (s.width as f32, s.height as f32)) };
    match size.filter(|&(w, h)| w > 0.0 && h > 0.0) {
        Some((width, height)) => Image::Local { path, width, height },
        None => Image::Unavailable { path: path.display().to_string(), reason: crate::i18n::text("无法读取图片", "Could not read image") },
    }
}

fn svg_size(data: &[u8]) -> Option<(f32, f32)> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    Some((tree.size().width(), tree.size().height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG signature plus an IHDR chunk: enough for the size probe.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn local_images_report_their_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img/a b.png"), png_header(640, 480)).unwrap();
        std::fs::write(dir.path().join("d.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="30"/>"#).unwrap();
        assert_eq!(
            resolve("img/a%20b.png", Some(dir.path())),
            Image::Local { path: dir.path().join("img/a b.png"), width: 640.0, height: 480.0 }
        );
        assert_eq!(resolve("d.svg", Some(dir.path())), Image::Local { path: dir.path().join("d.svg"), width: 120.0, height: 30.0 });
    }

    #[test]
    fn remote_and_unavailable_images() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.webp"), "RIFF").unwrap();
        std::fs::write(dir.path().join("bad.png"), "not a png").unwrap();
        assert_eq!(resolve("https://e.com/a.png", Some(dir.path())), Image::Remote("https://e.com/a.png".into()));
        let reason = |src| match resolve(src, Some(dir.path())) {
            Image::Unavailable { path, reason } => (path, reason),
            other => panic!("{other:?}"),
        };
        assert_eq!(reason("nope.png"), ("nope.png".into(), "找不到图片"));
        assert_eq!(reason("x.webp").1, "不支持的图片格式");
        assert_eq!(reason("bad.png").1, "无法读取图片");
        assert!(matches!(resolve("a.png", None), Image::Unavailable { .. }), "inline content has no directory");
    }

    #[test]
    fn unavailable_reasons_read_in_english() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.webp"), "RIFF").unwrap();
        std::fs::write(dir.path().join("bad.png"), "not a png").unwrap();
        let reason = |src| match resolve(src, Some(dir.path())) {
            Image::Unavailable { reason, .. } => reason,
            other => panic!("{other:?}"),
        };
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(reason("nope.png"), "Image not found");
            assert_eq!(reason("x.webp"), "Unsupported image format");
            assert_eq!(reason("bad.png"), "Could not read image");
        });
    }

    #[test]
    fn remote_images_are_located_without_io_and_sized_from_bytes() {
        let base = Path::new("/home/dev/r");
        assert_eq!(locate("img/a%20b.png", Some(base)), Ok(base.join("img/a b.png")));
        assert_eq!(locate("https://e.com/a.png", Some(base)), Err(Image::Remote("https://e.com/a.png".into())));
        assert!(matches!(locate("a.png", None), Err(Image::Unavailable { .. })));
        assert!(supported(Path::new("/x/a.PNG")) && supported(Path::new("/x/d.svg")) && !supported(Path::new("/x/a.webp")));
        let cached = PathBuf::from("/tmp/cache/1.png");
        assert_eq!(from_bytes(cached.clone(), &png_header(64, 32)), Image::Local { path: cached.clone(), width: 64.0, height: 32.0 });
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="3"/>"#;
        assert_eq!(from_bytes(PathBuf::from("/tmp/cache/2.svg"), svg), Image::Local { path: "/tmp/cache/2.svg".into(), width: 12.0, height: 3.0 });
        assert!(matches!(from_bytes(cached, b"not a png"), Image::Unavailable { .. }));
    }
}
