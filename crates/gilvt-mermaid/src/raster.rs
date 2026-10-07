//! SVG to pixels with resvg, using the system fonts (CJK included).

use std::sync::OnceLock;

use resvg::{tiny_skia, usvg};

/// Largest raster side in device pixels; bigger diagrams are refused rather than allocated.
const MAX_SIDE: u32 = 16_384;

/// Premultiplied RGBA8, row-major, `width * height * 4` bytes.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Parsing options with system fonts loaded once. `<image>` hrefs other than data URLs are
/// ignored, so an SVG (e.g. from a tampered cache) cannot read local files.
fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = usvg::Options::default();
        options.fontdb_mut().load_system_fonts();
        options.image_href_resolver.resolve_string = Box::new(|_, _| None);
        options
    })
}

/// Rasterizes at `scale` (device pixels per SVG unit).
pub fn rasterize(svg: &str, scale: f32) -> Result<Raster, String> {
    let tree = usvg::Tree::from_str(svg, options()).map_err(|e| e.to_string())?;
    let size = usvg::Size::from_wh(tree.size().width() * scale, tree.size().height() * scale)
        .map(|size| size.to_int_size())
        .filter(|size| size.width() <= MAX_SIDE && size.height() <= MAX_SIDE)
        .ok_or_else(|| format!("diagram too large to draw at scale {scale}"))?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| "diagram has an empty size".to_string())?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(Raster { width: pixmap.width(), height: pixmap.height(), rgba: pixmap.take() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
        <rect x="5" y="5" width="30" height="10" fill="#c00"/>
        <text x="2" y="18" font-size="6" font-family="sans-serif">Hi 你好</text>
    </svg>"##;

    #[test]
    fn size_follows_scale() {
        let one = rasterize(SVG, 1.0).unwrap();
        assert_eq!((one.width, one.height), (40, 20));
        assert_eq!(one.rgba.len(), 40 * 20 * 4);
        let two = rasterize(SVG, 2.0).unwrap();
        assert_eq!((two.width, two.height), (80, 40));
    }

    #[test]
    fn draws_something() {
        let raster = rasterize(SVG, 2.0).unwrap();
        assert!(raster.rgba.chunks_exact(4).any(|px| px[3] != 0));
    }

    #[test]
    fn rejects_garbage_and_oversize() {
        assert!(rasterize("not an svg", 1.0).is_err());
        assert!(rasterize(SVG, 1000.0).is_err());
    }
}
