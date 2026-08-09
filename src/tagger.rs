use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use crate::apple::{Album, Artwork, Track};
use crate::flac::Audio;

const ARTWORK_LIMIT: u64 = 32 * 1024 * 1024;
const ARTWORK_UNKNOWN: u32 = 10_000;

pub struct Picture {
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct Item {
    pub audio: Audio,
    pub disc: u16,
    pub track: u16,
    pub title: String,
    pub duration_ms: u64,
    pub positional: bool,
    pub comments: Vec<(String, Vec<String>)>,
}

pub enum Change {
    Added(String, String),
    Updated(String, String, String),
    Removed(String, String),
}

pub fn plan(album: &Album, songs: HashMap<String, PathBuf>) -> Result<Vec<Item>> {
    let mut sources: Vec<(String, Audio)> = Vec::with_capacity(songs.len());
    for (isrc, path) in songs {
        sources.push((isrc, Audio::open(path)?));
    }
    sources.sort_by(|left, right| left.1.path.cmp(&right.1.path));

    let mut claimed = vec![false; album.tracks.len()];
    let mut assigned: Vec<Option<usize>> = vec![None; sources.len()];

    for (index, (isrc, _)) in sources.iter().enumerate() {
        let found = album.tracks.iter().position(|track| {
            track
                .isrc
                .as_deref()
                .is_some_and(|found| found.eq_ignore_ascii_case(isrc))
        });

        if let Some(found) = found
            && !claimed[found]
        {
            claimed[found] = true;
            assigned[index] = Some(found);
        }
    }

    let mut positional = vec![false; sources.len()];

    for (index, (_, audio)) in sources.iter().enumerate() {
        if assigned[index].is_some() {
            continue;
        }

        let Some(number) = audio.number("TRACKNUMBER") else {
            continue;
        };
        let disc = audio.number("DISCNUMBER").unwrap_or(1).max(1);

        let found = album
            .tracks
            .iter()
            .position(|track| track.disc_number == disc && track.track_number == number);

        if let Some(found) = found
            && !claimed[found]
        {
            claimed[found] = true;
            assigned[index] = Some(found);
            positional[index] = true;
        }
    }

    let mut items = Vec::with_capacity(sources.len());

    for (index, (isrc, audio)) in sources.into_iter().enumerate() {
        let matched = assigned[index].ok_or_else(|| {
            anyhow!(
                "{} carries {isrc}, and {} has no track with that isrc or a free ({}, {}) slot",
                audio.path.display(),
                album.name,
                audio.number("DISCNUMBER").unwrap_or(1),
                audio
                    .number("TRACKNUMBER")
                    .map_or_else(|| "?".to_owned(), |number| number.to_string()),
            )
        })?;
        let matched = &album.tracks[matched];

        let comments = comments(album, matched, &audio);

        items.push(Item {
            audio,
            disc: matched.disc_number,
            track: matched.track_number,
            title: matched.name.clone(),
            duration_ms: matched.duration_ms,
            positional: positional[index],
            comments,
        });
    }

    items.sort_by_key(|item| (item.disc, item.track));

    Ok(items)
}

pub fn changes(item: &Item) -> Vec<Change> {
    let after: BTreeMap<&str, String> = item
        .comments
        .iter()
        .map(|(key, values)| (key.as_str(), join(values)))
        .collect();

    let mut out = Vec::new();

    for (key, value) in &after {
        match item.audio.comments.get(*key).map(|values| join(values)) {
            Some(before) if &before == value => {}
            Some(before) => out.push(Change::Updated((*key).to_owned(), before, value.clone())),
            None => out.push(Change::Added((*key).to_owned(), value.clone())),
        }
    }

    for (key, values) in &item.audio.comments {
        if !after.contains_key(key.as_str()) {
            out.push(Change::Removed(key.clone(), join(values)));
        }
    }

    out
}

pub fn drift(item: &Item) -> Option<u64> {
    let expected = (item.duration_ms + 500) / 1000;
    let actual = item.audio.duration_secs?;
    Some(expected.abs_diff(actual))
}

pub fn artwork(art: &Artwork) -> Result<Picture> {
    let width = if art.width == 0 {
        ARTWORK_UNKNOWN
    } else {
        art.width
    };
    let height = if art.height == 0 {
        ARTWORK_UNKNOWN
    } else {
        art.height
    };
    let url = art.url(width, height);

    let mut response = ureq::get(&url)
        .call()
        .with_context(|| format!("downloading artwork from {url}"))?;

    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map_or_else(
            || "image/jpeg".to_owned(),
            |value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or("image/jpeg")
                    .trim()
                    .to_owned()
            },
        );

    let data = response
        .body_mut()
        .with_config()
        .limit(ARTWORK_LIMIT)
        .read_to_vec()
        .with_context(|| format!("reading artwork from {url}"))?;

    Ok(Picture {
        mime,
        width,
        height,
        data,
    })
}

fn comments(album: &Album, track: &Track, audio: &Audio) -> Vec<(String, Vec<String>)> {
    let mut out = Builder::default();

    out.set("TITLE", track.name.clone());
    out.set("ARTIST", track.artist.clone());
    out.many("ARTISTS", track.artists.clone());
    out.set("ALBUM", album.name.clone());
    out.set(
        "ALBUMARTIST",
        track
            .album_artist
            .clone()
            .unwrap_or_else(|| album.artist.clone()),
    );
    out.many("ALBUMARTISTS", album.artists.clone());
    out.set("TRACKNUMBER", track.track_number.to_string());
    out.set("DISCNUMBER", track.disc_number.to_string());

    out.maybe("DATE", album.release_date.clone());
    if let Some(year) = album.release_date.as_ref().and_then(|date| date.get(..4)) {
        out.set("YEAR", year.to_owned());
    }

    out.maybe("WORK", track.work.clone());
    out.maybe("MOVEMENTNAME", track.movement.clone());
    if track.movement_number > 0 {
        out.set("MOVEMENT", track.movement_number.to_string());
    }
    if track.movement_count > 0 {
        out.set("MOVEMENTTOTAL", track.movement_count.to_string());
    }

    out.maybe("ISRC", audio.first("ISRC").or_else(|| track.isrc.clone()));
    out.maybe(
        "BARCODE",
        audio
            .first("BARCODE")
            .or_else(|| audio.first("UPC"))
            .or_else(|| album.upc.clone()),
    );

    if let Some(advisory) = advisory(track.content_rating.as_deref()) {
        out.set("ITUNESADVISORY", advisory);
    }

    if album.is_compilation {
        out.set("COMPILATION", "1");
    }

    out.0
}

fn advisory(rating: Option<&str>) -> Option<&'static str> {
    match rating? {
        "explicit" => Some("1"),
        "clean" => Some("2"),
        _ => None,
    }
}

fn join(values: &[String]) -> String {
    values.join(" / ")
}

#[derive(Default)]
struct Builder(Vec<(String, Vec<String>)>);

impl Builder {
    fn set(&mut self, key: &str, value: impl Into<String>) {
        self.0.push((key.to_owned(), vec![value.into()]));
    }

    fn maybe(&mut self, key: &str, value: Option<String>) {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            self.set(key, value);
        }
    }

    fn many(&mut self, key: &str, values: Vec<String>) {
        if !values.is_empty() {
            self.0.push((key.to_owned(), values));
        }
    }
}
