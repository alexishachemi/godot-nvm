use std::{fs, path::Path};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbaImage};

pub fn load(path: &Path) -> Result<DynamicImage> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    {
        load_svg(path)
    } else {
        image::open(path).with_context(|| format!("could not decode {}", path.display()))
    }
}

fn load_svg(path: &Path) -> Result<DynamicImage> {
    let data = fs::read(path)?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .context("SVG has invalid dimensions")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    let image = RgbaImage::from_raw(size.width(), size.height(), pixmap.take())
        .context("SVG renderer returned an invalid image")?;
    Ok(DynamicImage::ImageRgba8(image))
}

#[cfg(test)]
mod tests {
    use image::{GenericImageView, ImageFormat, Rgba};

    use super::*;

    #[test]
    fn loads_a_png_icon() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();

        assert_eq!(load(&path).unwrap().dimensions(), (3, 2));
    }
}
