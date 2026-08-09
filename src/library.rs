use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::apple::Album;
use crate::tagger::{Item, Picture};

const COVER: &str = "cover";
const UNTITLED: &str = "Untitled";
const COMPONENT_LIMIT: usize = 200;

pub fn root() -> Result<PathBuf> {
    dirs::audio_dir().ok_or_else(|| anyhow!("no audio directory; set XDG_MUSIC_DIR"))
}

pub fn album_dir(root: &Path, album: &Album) -> PathBuf {
    root.join(sanitize(&album.artist))
        .join(sanitize(&album.name))
}

pub fn track_path(dir: &Path, album: &Album, item: &Item) -> PathBuf {
    let title = sanitize(&item.title);

    if album.disc_count > 1 {
        dir.join(format!("{}-{:02} {title}.flac", item.disc, item.track))
    } else {
        dir.join(format!("{:02} {title}.flac", item.track))
    }
}

pub fn cover(dir: &Path, picture: &Picture) -> Result<PathBuf> {
    let path = dir.join(format!("{COVER}.{}", image_extension(&picture.mime)));

    fs::write(&path, &picture.data)
        .with_context(|| format!("writing the cover to {}", path.display()))?;

    Ok(path)
}

pub fn same_file(left: &Path, right: &Path) -> Result<bool> {
    let left = left
        .canonicalize()
        .with_context(|| format!("resolving {}", left.display()))?;
    let right = right
        .canonicalize()
        .with_context(|| format!("resolving {}", right.display()))?;

    Ok(left == right)
}

fn sanitize(text: &str) -> String {
    let name = sanitize_filename::sanitize_with_options(
        text,
        sanitize_filename::Options {
            windows: true,
            truncate: false,
            replacement: "-",
        },
    );

    let kept = if name.len() > COMPONENT_LIMIT {
        let mut end = COMPONENT_LIMIT;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name[..end].trim_end_matches(['.', ' '])
    } else {
        name.as_str()
    };

    if kept.is_empty() {
        UNTITLED.to_owned()
    } else {
        kept.to_owned()
    }
}

fn image_extension(mime: &str) -> String {
    let subtype = mime.rsplit('/').next().unwrap_or_default().trim();

    match subtype {
        "jpeg" => "jpg".to_owned(),
        other if !other.is_empty() && other.chars().all(|char| char.is_ascii_alphanumeric()) => {
            other.to_ascii_lowercase()
        }
        _ => "jpg".to_owned(),
    }
}
