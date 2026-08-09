use anyhow::Result;
use dialoguer::Select;
use dialoguer::theme::ColorfulTheme;

use crate::apple::Album;
use crate::flac::Skipped;
use crate::select::Candidate;
use crate::tagger::{self, Change, Item};

const SKIP: &str = "skip this album";

pub fn prompt(candidates: &[Candidate], files: usize) -> Result<Option<usize>> {
    let mut items: Vec<String> = candidates
        .iter()
        .map(|candidate| {
            format!(
                "[{}{}] {}",
                candidate.source.label(),
                if candidate.isrc_hits > 0 {
                    format!(" {}/{files}", candidate.isrc_hits)
                } else {
                    String::new()
                },
                summary(&candidate.album)
            )
        })
        .collect();
    items.push(SKIP.to_owned());

    let picked = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "{files} files, {} candidate(s), none confirmed by barcode",
            candidates.len()
        ))
        .items(&items)
        .default(0)
        .interact()?;

    Ok((picked < candidates.len()).then_some(picked))
}

pub fn summary(album: &Album) -> String {
    format!(
        "{} — {} ({} tracks, {}, upc {}, id {})",
        album.artist,
        album.name,
        album.tracks.len(),
        album.release_date.as_deref().unwrap_or("-"),
        album.upc.as_deref().unwrap_or("-"),
        album.id
    )
}

pub fn skipped(skipped: &[Skipped]) {
    if skipped.is_empty() {
        return;
    }

    println!("skipped {} files:", skipped.len());
    for skip in skipped {
        match skip {
            Skipped::NoBarcode(path) => println!("  no barcode   {}", path.display()),
            Skipped::NoIsrc(path) => println!("  no isrc      {}", path.display()),
            Skipped::DuplicateIsrc(path, isrc) => {
                println!("  {isrc} twice  {}", path.display());
            }
        }
    }
    println!();
}

pub fn report(item: &Item, verbose: bool) {
    let changes = tagger::changes(item);

    println!(
        "{}-{:02}  {}  →  {}",
        item.disc,
        item.track,
        truncate(&item.audio.name(), 46),
        item.title
    );

    if item.positional {
        println!("      ! matched by (disc, track), not by isrc");
    }

    if let Some(drift) = tagger::drift(item)
        && drift > 5
    {
        println!("      ! runs {drift}s away from the apple track length");
    }

    if verbose {
        for change in &changes {
            match change {
                Change::Added(key, value) => {
                    println!("      + {key:<16} {}", truncate(value, 60));
                }
                Change::Updated(key, before, after) => {
                    println!(
                        "      ~ {key:<16} {}  →  {}",
                        truncate(before, 28),
                        truncate(after, 28)
                    );
                }
                Change::Removed(key, value) => {
                    println!("      - {key:<16} {}", truncate(value, 60));
                }
            }
        }
    } else {
        println!("      {} tag changes", changes.len());
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}
