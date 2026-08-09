# redbook

A command-line tagger for FLAC rips. It identifies each album against the Apple
Music catalog, rewrites the Vorbis comments from the catalog metadata, and files
the result into your music library.

It is built around the disc barcode and ISRC.
Guessing based on existing file and album names is not supported (or intended).

This is primarily a personal project, built for my own library and my own
workflow. It is deliberately not very configurable: the tag set, the naming
scheme, and the directory layout are what I want them to be, not options. You are
welcome to use it, but expect to fork it rather than configure it.

## Installation

```
cargo install --git https://github.com/Graukolos/redbook
```

## Usage

```
redbook [OPTIONS] [SOURCES]...
```

Point it at directories (or individual files) holding freshly ripped FLACs:

```
redbook ~/rips/incoming
```

| Option | Effect |
| --- | --- |
| `--dry-run` | Resolve and report everything, write nothing. Also prints the full per-tag diff. |
| `--copy` | Copy the source files instead of moving them. |
| `--storefront <CODE>` | Apple Music storefront to query. Defaults to `us`. |

## How it works

**1. Collect.** Every `.flac` under the given paths is read. A file needs a
`BARCODE` (or `UPC`) tag and an `ISRC` tag; anything missing one is reported and
skipped. Files are grouped by barcode, so one group is one album.

**2. Identify.** For each group, redbook asks the catalog in order:

- look up every ISRC and count which albums carry them,
- if nothing came back, filter albums by the barcode,
- if no candidate is convincing yet, fall back to a text search on
  artist + album, filtered by artist/title similarity and track count.

Candidates are ranked by ISRC hit count, barcode agreement, and how close the
track count is. The top candidate is accepted automatically when the barcode
matches the catalog UPC, or when every file's ISRC hit that same album. Otherwise
you get an interactive picker listing each candidate with the evidence behind it,
plus an option to skip the album.

**3. Match tracks.** Files are paired to catalog tracks by ISRC first. Anything
left over falls back to matching on `(DISCNUMBER, TRACKNUMBER)` against a free
slot, and those tracks are flagged in the output. A file that matches neither way
is an error and stops the run.

**4. Write.** The tagged file is built in a temp file next to its destination and
then moved into place, so a source file is never edited in situ. Cover art is
downloaded at full resolution and saved next to the tracks as `cover.jpg`.

## Output layout

Albums land under your XDG music directory (`XDG_MUSIC_DIR`, usually `~/Music`):

```
~/Music/<Album Artist>/<Album>/01 Track Title.flac
~/Music/<Album Artist>/<Album>/cover.jpg
```

Multi-disc releases are prefixed with the disc number instead:
`1-01 Track Title.flac`. Path components are sanitized and capped at 200 bytes.

## Tags

Existing metadata is **replaced, not merged**. Every metadata block except
`STREAMINFO` is dropped — including embedded artwork, cuesheets, and any tag not
in the list below — and then these are written from the catalog:

`TITLE`, `ARTIST`, `ARTISTS`, `ALBUM`, `ALBUMARTIST`, `ALBUMARTISTS`,
`TRACKNUMBER`, `DISCNUMBER`, `DATE`, `YEAR`, `WORK`, `MOVEMENTNAME`, `MOVEMENT`,
`MOVEMENTTOTAL`, `ISRC`, `BARCODE`, `ITUNESADVISORY`, `COMPILATION`.

`ISRC` and `BARCODE` keep the value already on the file, falling back to the
catalog's. Fields that do not apply (classical work/movement data, advisory,
compilation) are simply omitted.

The mapping is tuned for [Navidrome](https://www.navidrome.org). In particular,
`ARTIST` and `ALBUMARTIST` hold the full display credit as a single string — the
whole "A & B feat. C" — while `ARTISTS` and `ALBUMARTISTS` are multi-valued and
enumerate the participants one name per value. Navidrome uses the plural tags to
build its artist index and the singular ones for display, so credits stay
readable without fragmenting the artist list. Other players may read these
differently.

Run with `--dry-run` first to see exactly which tags would be added, changed, or
removed.

## Apple Music access

No Apple account or developer program membership is needed. redbook scrapes the
public web player's anonymous developer token from its JavaScript bundle and
caches it in `~/.cache/redbook/apple-token.json` until it expires. If a request
comes back unauthorized, the cached token is discarded and re-scraped once.

## License

MIT — see [LICENSE](LICENSE).
