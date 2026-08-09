use std::collections::HashMap;

use anyhow::Result;

use crate::apple::{Album, Catalog};

const CANDIDATE_LIMIT: usize = 8;
const SEARCH_LIMIT: usize = 15;
const TRACK_SLACK: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Isrc,
    Barcode,
    Search,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Isrc => "isrc",
            Self::Barcode => "barcode",
            Self::Search => "search",
        }
    }
}

pub struct Hint<'a> {
    pub upc: &'a str,
    pub album: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub files: usize,
}

pub struct Candidate {
    pub album: Album,
    pub source: Source,
    pub isrc_hits: usize,
}

impl Candidate {
    fn rank(&self, hint: &Hint) -> (bool, usize, bool, usize) {
        (
            !self.plausible(hint),
            usize::MAX - self.isrc_hits,
            !self.album.matches_barcode(hint.upc),
            self.album.tracks.len().abs_diff(hint.files),
        )
    }

    fn plausible(&self, hint: &Hint) -> bool {
        self.album.tracks.len() <= hint.files * 2
    }

    fn confident(&self, hint: &Hint) -> bool {
        if self.album.matches_barcode(hint.upc) {
            return true;
        }

        self.isrc_hits == hint.files && self.plausible(hint)
    }
}

pub fn candidates(catalog: &Catalog, isrcs: &[String], hint: &Hint) -> Result<Vec<Candidate>> {
    let mut found = by_isrc(catalog, isrcs)?;

    if found.is_empty() {
        found = by_barcode(catalog, hint)?;
    }

    if !found.iter().any(|candidate| candidate.confident(hint)) {
        for extra in by_search(catalog, hint)? {
            if !found
                .iter()
                .any(|candidate| candidate.album.id == extra.album.id)
            {
                found.push(extra);
            }
        }
    }

    found.sort_by_key(|candidate| candidate.rank(hint));

    Ok(found)
}

pub fn choose(candidates: &[Candidate], hint: &Hint) -> Option<usize> {
    candidates
        .first()
        .is_some_and(|candidate| candidate.confident(hint))
        .then_some(0)
}

fn by_isrc(catalog: &Catalog, isrcs: &[String]) -> Result<Vec<Candidate>> {
    let carried = catalog.album_ids_by_isrcs(isrcs)?;

    let mut hits: HashMap<String, usize> = HashMap::new();
    for isrc in isrcs {
        let Some(ids) = carried.get(isrc) else {
            continue;
        };
        for id in ids {
            *hits.entry(id.clone()).or_default() += 1;
        }
    }

    let threshold = hits.values().copied().max().unwrap_or(0).max(1);
    let mut ranked: Vec<(usize, String)> = hits
        .into_iter()
        .filter(|(_, hits)| *hits * 2 >= isrcs.len() || *hits >= threshold)
        .map(|(id, hits)| (hits, id))
        .collect();

    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    ranked.truncate(CANDIDATE_LIMIT);

    let mut out = Vec::with_capacity(ranked.len());
    for (hits, id) in ranked {
        out.push(Candidate {
            album: catalog.album(&id)?,
            source: Source::Isrc,
            isrc_hits: hits,
        });
    }

    Ok(out)
}

fn by_barcode(catalog: &Catalog, hint: &Hint) -> Result<Vec<Candidate>> {
    let mut ids = catalog.album_ids_by_upc(hint.upc)?;
    ids.truncate(CANDIDATE_LIMIT);

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(Candidate {
            album: catalog.album(&id)?,
            source: Source::Barcode,
            isrc_hits: 0,
        });
    }

    Ok(out)
}

fn by_search(catalog: &Catalog, hint: &Hint) -> Result<Vec<Candidate>> {
    let Some(term) = term(hint) else {
        return Ok(Vec::new());
    };

    let mut found = catalog.search_albums(&term, SEARCH_LIMIT)?;

    found.retain(|album| {
        usize::from(album.track_count) >= hint.files.saturating_sub(TRACK_SLACK)
            && hint
                .artist
                .is_none_or(|artist| similar(&album.artist, artist))
            && hint.album.is_none_or(|name| similar(&album.name, name))
    });

    found.sort_by_key(|album| {
        (
            !album.matches_barcode(hint.upc),
            usize::from(album.track_count).abs_diff(hint.files),
        )
    });
    found.truncate(CANDIDATE_LIMIT);

    let mut out = Vec::with_capacity(found.len());
    for album in found {
        out.push(Candidate {
            album: catalog.album(&album.id)?,
            source: Source::Search,
            isrc_hits: 0,
        });
    }

    Ok(out)
}

fn term(hint: &Hint) -> Option<String> {
    match (hint.artist, hint.album) {
        (Some(artist), Some(album)) => Some(format!("{artist} {album}")),
        (None, Some(album)) => Some(album.to_owned()),
        _ => None,
    }
}

fn similar(left: &str, right: &str) -> bool {
    let fold = |text: &str| {
        text.chars()
            .filter(|char| char.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let left = fold(left);
    let right = fold(right);

    !left.is_empty() && !right.is_empty() && (left.contains(&right) || right.contains(&left))
}
