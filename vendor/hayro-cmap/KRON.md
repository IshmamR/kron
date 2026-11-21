# hayro-cmap, patched for Kron

A copy of [hayro-cmap](https://github.com/LaurenzV/hayro) 0.1.0 by Laurenz
Stampfl, as published on crates.io, under its licences (`LICENSE-MIT`,
`LICENSE-APACHE`). Kron's `Cargo.toml` uses it in place of the published crate
through `[patch.crates-io]`.

## The change

`src/parse.rs`: a `bfchar` or `bfrange` entry whose destination is an empty
string (`<65> <>`) is skipped. Before, it made the whole CMap fail to parse.
Figma's PDF export writes such an entry in the ToUnicode CMap of its Type 3
fonts, so a CV made in Figma read as having no text at all. The changed lines
are marked `// Kron:`.

The latest release, 0.1.1, has the same code. Delete this folder and the
`[patch.crates-io]` entry once a hayro-cmap release skips such entries.
