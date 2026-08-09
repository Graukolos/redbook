use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

mod apple;
mod flac;
mod library;
mod select;
mod tagger;
mod ui;

#[derive(Parser)]
struct Cli {
    sources: Vec<PathBuf>,

    #[arg(long)]
    dry_run: bool,

    #[arg(long)]
    copy: bool,

    #[arg(long, default_value = "us")]
    storefront: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let catalog = apple::Catalog::with_storefront(&cli.storefront);
    let root = library::root()?;

    let collected = flac::collect(&cli.sources)?;
    ui::skipped(&collected.skipped);

    for (upc, group) in collected.albums {
        let isrcs: Vec<String> = group.songs.keys().cloned().collect();
        let hint = select::Hint {
            upc: &upc,
            album: group.album.as_deref(),
            artist: group.artist.as_deref(),
            files: group.songs.len(),
        };

        let candidates = select::candidates(&catalog, &isrcs, &hint)?;
        if candidates.is_empty() {
            println!(
                "upc {upc}: no album found by isrc, barcode or search for {}\n",
                group.album.as_deref().unwrap_or("these files")
            );
            continue;
        }

        let picked = match select::choose(&candidates, &hint) {
            Some(index) => Some(index),
            None => ui::prompt(&candidates, hint.files)?,
        };
        let Some(index) = picked else {
            println!("skipped\n");
            continue;
        };
        let album = &candidates[index].album;

        let items = tagger::plan(album, group.songs)?;
        let dir = library::album_dir(&root, album);

        println!("{}", ui::summary(album));
        println!("into      {}", dir.display());

        let picture = match (cli.dry_run, &album.artwork) {
            (false, Some(art)) => Some(tagger::artwork(art)?),
            _ => None,
        };
        if let Some(picture) = &picture {
            println!(
                "cover art  {}×{} {}, {} KiB",
                picture.width,
                picture.height,
                picture.mime,
                picture.data.len() / 1024
            );
        }
        println!();

        for item in &items {
            ui::report(item, cli.dry_run);
        }

        if cli.dry_run {
            println!("dry run: nothing written");
            continue;
        }

        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        for item in &items {
            let dest = library::track_path(&dir, album, item);
            flac::save(&item.audio.path, &dest, &item.comments)?;

            if !cli.copy && !library::same_file(&item.audio.path, &dest)? {
                fs::remove_file(&item.audio.path).with_context(|| {
                    format!("removing {} after the move", item.audio.path.display())
                })?;
            }
        }

        println!(
            "{} {} files",
            if cli.copy { "copied" } else { "moved" },
            items.len()
        );

        if let Some(picture) = &picture {
            let path = library::cover(&dir, picture)?;
            println!("wrote {}", path.display());
        }
    }

    Ok(())
}
