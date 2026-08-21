use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::USER_AGENT;
use super::token::TokenStore;

const API: &str = "https://amp-api.music.apple.com/v1";
const ORIGIN: &str = "https://music.apple.com";
const BODY_LIMIT: u64 = 16 * 1024 * 1024;
const ISRC_BATCH: usize = 25;

pub struct Catalog {
    agent: ureq::Agent,
    tokens: TokenStore,
    storefront: String,
    albums: Mutex<HashMap<String, Album>>,
    songs: Mutex<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub artists: Vec<String>,
    pub upc: Option<String>,
    pub release_date: Option<String>,
    pub disc_count: u16,
    pub is_compilation: bool,
    pub artwork: Option<Artwork>,
    pub tracks: Vec<Track>,
}

impl Album {
    pub fn matches_barcode(&self, barcode: &str) -> bool {
        self.upc
            .as_deref()
            .is_some_and(|found| same_barcode(found, barcode))
    }
}

#[derive(Debug, Clone)]
pub struct Track {
    pub name: String,
    pub artist: String,
    pub artists: Vec<String>,
    pub album_artist: Option<String>,
    pub isrc: Option<String>,
    pub content_rating: Option<String>,
    pub work: Option<String>,
    pub movement: Option<String>,
    pub movement_number: u16,
    pub movement_count: u16,
    pub disc_number: u16,
    pub track_number: u16,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub track_count: u16,
    pub upc: Option<String>,
}

impl Found {
    pub fn matches_barcode(&self, barcode: &str) -> bool {
        self.upc
            .as_deref()
            .is_some_and(|found| same_barcode(found, barcode))
    }
}

#[derive(Debug, Clone)]
pub struct Artwork {
    template: String,
    pub width: u32,
    pub height: u32,
}

impl Artwork {
    pub fn url(&self, width: u32, height: u32) -> String {
        self.template
            .replace("{w}", &width.to_string())
            .replace("{h}", &height.to_string())
    }
}

impl Catalog {
    pub fn with_storefront(storefront: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();

        Self {
            agent: ureq::Agent::new_with_config(config),
            tokens: TokenStore::new(),
            storefront: storefront.to_owned(),
            albums: Mutex::new(HashMap::new()),
            songs: Mutex::new(HashMap::new()),
        }
    }

    pub fn album(&self, id: &str) -> Result<Album> {
        if let Some(album) = self.albums.lock().expect("album cache poisoned").get(id) {
            return Ok(album.clone());
        }

        let path = format!("/catalog/{}/albums/{id}", self.storefront);
        let response: Envelope<AlbumResource> = self.get(
            &path,
            &[("include", "tracks,artists"), ("include[songs]", "artists")],
        )?;

        let resource = response
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("apple: album {id} returned no data"))?;

        let album = self.build_album(resource)?;

        self.albums
            .lock()
            .expect("album cache poisoned")
            .insert(id.to_owned(), album.clone());

        Ok(album)
    }

    pub fn album_ids_by_upc(&self, upc: &str) -> Result<Vec<String>> {
        let path = format!("/catalog/{}/albums", self.storefront);
        let response: Envelope<AlbumStub> = self.get(&path, &[("filter[upc]", upc)])?;

        Ok(response.data.into_iter().map(|album| album.id).collect())
    }

    pub fn search_albums(&self, term: &str, limit: usize) -> Result<Vec<Found>> {
        let path = format!("/catalog/{}/search", self.storefront);
        let limit = limit.to_string();
        let response: SearchResponse = self.get(
            &path,
            &[("term", term), ("types", "albums"), ("limit", &limit)],
        )?;

        Ok(response
            .results
            .albums
            .map(|albums| albums.data)
            .unwrap_or_default()
            .into_iter()
            .map(|album| Found {
                id: album.id,
                name: album.attributes.name,
                artist: album.attributes.artist_name,
                track_count: album.attributes.track_count,
                upc: album.attributes.upc,
            })
            .collect())
    }

    pub fn album_ids_by_isrcs(&self, isrcs: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let mut found = HashMap::new();
        let mut pending = Vec::new();

        {
            let cache = self.songs.lock().expect("song cache poisoned");
            for isrc in isrcs {
                match cache.get(&isrc.to_uppercase()) {
                    Some(ids) => {
                        found.insert(isrc.clone(), ids.clone());
                    }
                    None => pending.push(isrc),
                }
            }
        }

        for batch in pending.chunks(ISRC_BATCH) {
            let mut ids: HashMap<String, Vec<String>> = batch
                .iter()
                .map(|isrc| (isrc.to_uppercase(), Vec::new()))
                .collect();

            let filter = batch
                .iter()
                .map(|isrc| isrc.as_str())
                .collect::<Vec<&str>>()
                .join(",");
            let path = format!("/catalog/{}/songs", self.storefront);
            let response: Envelope<SongResource> =
                self.get(&path, &[("filter[isrc]", &filter), ("include", "albums")])?;

            for song in response.data {
                let Some(isrc) = song.attributes.isrc else {
                    continue;
                };
                let Some(found) = ids.get_mut(&isrc.to_uppercase()) else {
                    continue;
                };
                let Some(relationship) = song.relationships.albums else {
                    continue;
                };

                found.extend(relationship.data.into_iter().map(|album| album.id));
                let mut next = relationship.next;

                while let Some(path) = next {
                    let page: Relationship<AlbumStub> = self
                        .get(path.trim_start_matches("/v1"), &[])
                        .with_context(|| format!("apple: paging albums via {path}"))?;
                    found.extend(page.data.into_iter().map(|album| album.id));
                    next = page.next;
                }
            }

            let mut cache = self.songs.lock().expect("song cache poisoned");
            for isrc in batch {
                let mut album_ids = ids.remove(&isrc.to_uppercase()).unwrap_or_default();
                album_ids.sort_unstable();
                album_ids.dedup();

                cache.insert(isrc.to_uppercase(), album_ids.clone());
                found.insert((*isrc).clone(), album_ids);
            }
        }

        Ok(found)
    }

    fn build_album(&self, resource: AlbumResource) -> Result<Album> {
        let relationships = resource.relationships;
        let mut tracks = Vec::new();
        let mut next = None;

        if let Some(relationship) = relationships.tracks {
            tracks.extend(relationship.data.into_iter().map(Track::from));
            next = relationship.next;
        }

        while let Some(path) = next {
            let page: Relationship<TrackResource> = self
                .get(path.trim_start_matches("/v1"), &[("include", "artists")])
                .with_context(|| format!("apple: paging tracks via {path}"))?;
            tracks.extend(page.data.into_iter().map(Track::from));
            next = page.next;
        }

        let attributes = resource.attributes;
        let disc_count = tracks.iter().map(|track| track.disc_number).max();

        Ok(Album {
            id: resource.id,
            name: attributes.name,
            artist: attributes.artist_name,
            artists: names(relationships.artists),
            upc: attributes.upc,
            release_date: attributes.release_date,
            disc_count: disc_count.unwrap_or(1).max(1),
            is_compilation: attributes.is_compilation,
            artwork: attributes.artwork.map(Artwork::from),
            tracks,
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let body = match self.fetch(path, query)? {
            Fetched::Ok(body) => body,
            Fetched::Unauthorized => {
                self.tokens.invalidate();
                match self.fetch(path, query)? {
                    Fetched::Ok(body) => body,
                    Fetched::Unauthorized => {
                        bail!("apple: {path} rejected a freshly scraped token (401)")
                    }
                    Fetched::Failed(status, body) => bail!(describe(path, status, &body)),
                }
            }
            Fetched::Failed(status, body) => bail!(describe(path, status, &body)),
        };

        serde_json::from_str(&body).with_context(|| format!("apple: decoding response from {path}"))
    }

    fn fetch(&self, path: &str, query: &[(&str, &str)]) -> Result<Fetched> {
        let url = format!("{API}{path}");
        let token = self.tokens.get()?;

        let mut response = self
            .agent
            .get(&url)
            .query_pairs(query.iter().copied())
            .header("Authorization", format!("Bearer {token}"))
            .header("Origin", ORIGIN)
            .header("User-Agent", USER_AGENT)
            .call()
            .with_context(|| format!("apple: requesting {url}"))?;

        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()
            .with_context(|| format!("apple: reading response from {url}"))?;

        Ok(match status {
            200..=299 => Fetched::Ok(body),
            401 | 403 => Fetched::Unauthorized,
            _ => Fetched::Failed(status, body),
        })
    }
}

enum Fetched {
    Ok(String),
    Unauthorized,
    Failed(u16, String),
}

fn describe(path: &str, status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<ErrorResponse>(body)
        .ok()
        .and_then(|errors| errors.errors.into_iter().next())
        .map(|error| match error.detail {
            Some(detail) => format!("{}: {detail}", error.title),
            None => error.title,
        });

    match detail {
        Some(detail) => format!("apple: {path} failed with HTTP {status} ({detail})"),
        None => format!("apple: {path} failed with HTTP {status}"),
    }
}

fn names(relationship: Option<Relationship<ArtistResource>>) -> Vec<String> {
    relationship.map_or_else(Vec::new, |artists| {
        artists
            .data
            .into_iter()
            .filter_map(|artist| artist.attributes.map(|attributes| attributes.name))
            .collect()
    })
}

fn same_barcode(left: &str, right: &str) -> bool {
    let digits = |code: &str| {
        code.trim()
            .trim_start_matches('0')
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
    };
    let left = digits(left);
    !left.is_empty() && left == digits(right)
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Deserialize)]
struct Relationship<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct AlbumResource {
    #[serde(default)]
    id: String,
    attributes: AlbumAttributes,
    #[serde(default)]
    relationships: AlbumRelationships,
}

#[derive(Deserialize, Default)]
struct AlbumRelationships {
    #[serde(default)]
    tracks: Option<Relationship<TrackResource>>,
    #[serde(default)]
    artists: Option<Relationship<ArtistResource>>,
}

#[derive(Deserialize, Default)]
struct TrackRelationships {
    #[serde(default)]
    artists: Option<Relationship<ArtistResource>>,
}

#[derive(Deserialize)]
struct ArtistResource {
    #[serde(default)]
    attributes: Option<ArtistAttributes>,
}

#[derive(Deserialize)]
struct ArtistAttributes {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlbumAttributes {
    name: String,
    artist_name: String,
    #[serde(default)]
    upc: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    is_compilation: bool,
    #[serde(default)]
    artwork: Option<ArtworkResource>,
}

#[derive(Deserialize)]
struct SongResource {
    #[serde(default)]
    attributes: SongAttributes,
    #[serde(default)]
    relationships: SongRelationships,
}

#[derive(Deserialize, Default)]
struct SongAttributes {
    #[serde(default)]
    isrc: Option<String>,
}

#[derive(Deserialize, Default)]
struct SongRelationships {
    #[serde(default)]
    albums: Option<Relationship<AlbumStub>>,
}

#[derive(Deserialize)]
struct AlbumStub {
    id: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: SearchResults,
}

#[derive(Deserialize, Default)]
struct SearchResults {
    #[serde(default)]
    albums: Option<Relationship<SearchAlbum>>,
}

#[derive(Deserialize)]
struct SearchAlbum {
    id: String,
    attributes: SearchAlbumAttributes,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchAlbumAttributes {
    name: String,
    artist_name: String,
    #[serde(default)]
    track_count: u16,
    #[serde(default)]
    upc: Option<String>,
}

#[derive(Deserialize)]
struct TrackResource {
    attributes: TrackAttributes,
    #[serde(default)]
    relationships: TrackRelationships,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackAttributes {
    name: String,
    artist_name: String,
    #[serde(default)]
    album_artist_name: Option<String>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    content_rating: Option<String>,
    #[serde(default)]
    work_name: Option<String>,
    #[serde(default)]
    movement_name: Option<String>,
    #[serde(default)]
    movement_number: u16,
    #[serde(default)]
    movement_count: u16,
    #[serde(default)]
    disc_number: u16,
    #[serde(default)]
    track_number: u16,
    #[serde(default)]
    duration_in_millis: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtworkResource {
    url: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(default = "Vec::new")]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    title: String,
    #[serde(default)]
    detail: Option<String>,
}

impl From<TrackResource> for Track {
    fn from(resource: TrackResource) -> Self {
        let attributes = resource.attributes;
        Self {
            name: attributes.name,
            artist: attributes.artist_name,
            artists: names(resource.relationships.artists),
            album_artist: attributes.album_artist_name,
            isrc: attributes.isrc,
            content_rating: attributes.content_rating,
            work: attributes.work_name,
            movement: attributes.movement_name,
            movement_number: attributes.movement_number,
            movement_count: attributes.movement_count,
            disc_number: attributes.disc_number.max(1),
            track_number: attributes.track_number,
            duration_ms: attributes.duration_in_millis,
        }
    }
}

impl From<ArtworkResource> for Artwork {
    fn from(resource: ArtworkResource) -> Self {
        Self {
            template: resource.url,
            width: resource.width,
            height: resource.height,
        }
    }
}
