use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use metaflac::{Block, BlockType, Tag};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

#[derive(Default)]
pub struct Collected {
    pub albums: HashMap<String, Group>,
    pub skipped: Vec<Skipped>,
}

#[derive(Default)]
pub struct Group {
    pub songs: HashMap<String, PathBuf>,
    pub album: Option<String>,
    pub artist: Option<String>,
}

pub enum Skipped {
    NoBarcode(PathBuf),
    NoIsrc(PathBuf),
    DuplicateIsrc(PathBuf, String),
}

pub struct Audio {
    pub path: PathBuf,
    pub comments: BTreeMap<String, Vec<String>>,
    pub duration_secs: Option<u64>,
}

impl Audio {
    pub fn open(path: PathBuf) -> Result<Self> {
        let tag = read_tag(&path)?;

        let mut comments = BTreeMap::new();
        if let Some(vorbis) = tag.vorbis_comments() {
            for (key, values) in &vorbis.comments {
                comments.insert(key.to_uppercase(), values.clone());
            }
        }

        let duration_secs = tag.get_streaminfo().and_then(|info| {
            (info.sample_rate > 0).then(|| info.total_samples / u64::from(info.sample_rate))
        });

        Ok(Self {
            path,
            comments,
            duration_secs,
        })
    }

    pub fn first(&self, key: &str) -> Option<String> {
        self.comments
            .get(key)
            .and_then(|values| values.first())
            .filter(|value| !value.trim().is_empty())
            .cloned()
    }

    pub fn number(&self, key: &str) -> Option<u16> {
        let value = self.first(key)?;
        let digits: String = value
            .trim()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();

        digits.parse().ok()
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

pub fn collect(dirs: &[PathBuf]) -> Result<Collected> {
    let mut found = Vec::new();
    for dir in dirs {
        for entry in WalkDir::new(dir).follow_links(true) {
            let entry = entry.with_context(|| format!("walking {}", dir.display()))?;
            if entry.file_type().is_file() && is_flac(entry.path()) {
                found.push(entry.into_path());
            }
        }
    }

    let mut collected = Collected::default();
    for file in found {
        let tag = read_tag(&file)?;

        let Some(upc) = first(&tag, "BARCODE").or_else(|| first(&tag, "UPC")) else {
            collected.skipped.push(Skipped::NoBarcode(file));
            continue;
        };
        let Some(isrc) = first(&tag, "ISRC") else {
            collected.skipped.push(Skipped::NoIsrc(file));
            continue;
        };

        let group = collected.albums.entry(upc).or_default();

        if group.album.is_none() {
            group.album = first(&tag, "ALBUM");
        }
        if group.artist.is_none() {
            group.artist = first(&tag, "ALBUMARTIST").or_else(|| first(&tag, "ARTIST"));
        }

        if let Some(replaced) = group.songs.insert(isrc.clone(), file) {
            collected
                .skipped
                .push(Skipped::DuplicateIsrc(replaced, isrc));
        }
    }

    Ok(collected)
}

fn first(tag: &Tag, key: &str) -> Option<String> {
    tag.get_vorbis(key)
        .and_then(|mut values| values.next())
        .map(str::to_owned)
}

fn read_tag(path: &Path) -> Result<Tag> {
    Tag::read_from_path(path)
        .with_context(|| format!("reading flac metadata from {}", path.display()))
}

fn is_flac(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("flac"))
}

pub fn save(source: &Path, dest: &Path, comments: &[(String, Vec<String>)]) -> Result<()> {
    let folder = dest.parent().unwrap_or_else(|| Path::new("."));

    let scratch = NamedTempFile::new_in(folder)
        .with_context(|| format!("creating a temporary file in {}", folder.display()))?;

    fs::copy(source, scratch.path())
        .with_context(|| format!("copying {} aside before tagging", source.display()))?;

    let permissions = fs::metadata(source)
        .with_context(|| format!("reading the mode of {}", source.display()))?
        .permissions();
    fs::set_permissions(scratch.path(), permissions)
        .with_context(|| format!("setting the mode of the copy of {}", source.display()))?;

    rewrite(scratch.path(), comments)?;

    scratch
        .persist(dest)
        .map_err(|error| error.error)
        .with_context(|| format!("writing the tagged file to {}", dest.display()))?;

    Ok(())
}

fn rewrite(path: &Path, comments: &[(String, Vec<String>)]) -> Result<()> {
    let mut tag = read_tag(path)?;

    let discarded: Vec<BlockType> = tag
        .blocks()
        .map(Block::block_type)
        .filter(|block| block != &BlockType::StreamInfo)
        .collect();

    for block in discarded {
        tag.remove_blocks(block);
    }

    let vorbis = tag.vorbis_comments_mut();
    for (key, values) in comments {
        vorbis.set(key.clone(), values.clone());
    }

    tag.save()
        .with_context(|| format!("writing flac metadata to {}", path.display()))
}
