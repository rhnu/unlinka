//! Windows icon resources rendered at each native size from one editable SVG.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use resvg::{tiny_skia, usvg};

const ICON_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 96, 128, 256];
const TARGET_SIZES: &[u32] = &[16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256];
const SCALES: &[u32] = &[100, 125, 150, 200, 250, 300, 400];
const LOGOS: &[(&str, u32)] = &[
    ("StoreLogo", 50),
    ("Square44x44Logo", 44),
    ("Square150x150Logo", 150),
];

struct Asset {
    name: String,
    bytes: Vec<u8>,
}

/// Regenerate tracked resources, or report all missing and outdated outputs.
pub(crate) fn run(check: bool) -> io::Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let source = root.join("assets/Unlinka.svg");
    let destination = root.join("assets/windows");
    let assets = render_assets(&fs::read(&source)?)?;
    if check {
        let mut outdated = Vec::new();
        for asset in &assets {
            let path = destination.join(&asset.name);
            match fs::read(&path) {
                Ok(bytes) if bytes == asset.bytes => {},
                Ok(_) => outdated.push(asset.name.as_str()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    outdated.push(asset.name.as_str());
                },
                Err(error) => return Err(error),
            }
        }
        if !outdated.is_empty() {
            return Err(io::Error::other(format!(
                "icon resources missing or outdated: {}; run cargo xtask assets",
                outdated.join(", ")
            )));
        }
        eprintln!(
            "Checked {} icon resources against {}",
            assets.len(),
            source.display()
        );
        Ok(())
    } else {
        write_assets(&destination, &assets)
    }
}

/// Packaging renders into its own output directory before compiling resources.
#[cfg(windows)]
pub(crate) fn generate(source: &Path, destination: &Path) -> io::Result<()> {
    let assets = render_assets(&fs::read(source)?)?;
    write_assets(destination, &assets)
}

fn write_assets(destination: &Path, assets: &[Asset]) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for asset in assets {
        fs::write(destination.join(&asset.name), &asset.bytes)?;
    }
    eprintln!(
        "Generated {} icon resources in {}",
        assets.len(),
        destination.display()
    );
    Ok(())
}

fn render_assets(svg: &[u8]) -> io::Result<Vec<Asset>> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).map_err(io::Error::other)?;
    let dimensions = tree.size();
    if (dimensions.width() - dimensions.height()).abs()
        > f32::EPSILON * dimensions.width().max(dimensions.height())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the icon SVG must have a square canvas",
        ));
    }
    let mut assets = Vec::new();
    for &(name, base_size) in LOGOS {
        assets.push(png(&tree, format!("{name}.png"), base_size)?);
        for &scale in SCALES {
            // Windows rounds fractional scale dimensions to the nearest pixel.
            let size = (base_size * scale + 50) / 100;
            assets.push(png(&tree, format!("{name}.scale-{scale}.png"), size)?);
        }
    }
    for &size in TARGET_SIZES {
        let bytes = render(&tree, size)?
            .encode_png()
            .map_err(io::Error::other)?;
        // The high-contrast black plate and white mark work in both themes.
        for suffix in ["", "_altform-unplated", "_altform-lightunplated"] {
            assets.push(Asset {
                name: format!("Square44x44Logo.targetsize-{size}{suffix}.png"),
                bytes: bytes.clone(),
            });
        }
    }
    assets.push(png(&tree, "Unlinka.png".into(), 256)?);
    assets.push(Asset {
        name: "Unlinka.ico".into(),
        bytes: icon(&tree)?,
    });
    Ok(assets)
}

fn png(tree: &usvg::Tree, name: String, size: u32) -> io::Result<Asset> {
    Ok(Asset {
        name,
        bytes: render(tree, size)?.encode_png().map_err(io::Error::other)?,
    })
}

fn render(tree: &usvg::Tree, size: u32) -> io::Result<tiny_skia::Pixmap> {
    let size = u16::try_from(size).map_err(io::Error::other)?;
    let mut pixmap = tiny_skia::Pixmap::new(u32::from(size), u32::from(size))
        .ok_or_else(|| io::Error::other("cannot allocate icon canvas"))?;
    let scale = f32::from(size) / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn icon(tree: &usvg::Tree) -> io::Result<Vec<u8>> {
    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICON_SIZES {
        let pixmap = render(tree, size)?;
        // tiny-skia stores premultiplied color; ICO expects straight RGBA.
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        let entry = if size == 256 {
            ico::IconDirEntry::encode_as_png(&image)?
        } else {
            ico::IconDirEntry::encode_as_bmp(&image)?
        };
        directory.add_entry(entry);
    }
    let mut bytes = Vec::new();
    directory.write(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
