# hayro-cmap, patched for Kron

A copy of [hayro-cmap](https://github.com/LaurenzV/hayro) 0.1.0 by Laurenz
Stampfl, as published on crates.io, under its licences (`LICENSE-MIT`,
`LICENSE-APACHE`). Kron's `Cargo.toml` uses it in place of the published crate
through `[patch.crates-io]`.

## The change

A `bfchar` or `bfrange` entry whose destination is an empty string
(`<65> <>`) maps its code to an empty string (`src/parse.rs`, `decode_be`),
and looking it up gives `BfString::String("")` (`src/lib.rs`,
`decode_utf16`). Before, such an entry made the whole CMap fail to parse.
Figma's PDF export writes them in the ToUnicode CMaps of its Type 3 fonts,
for glyphs that draw nothing, so a CV made in Figma read as having no text at
all. Kron's PDF reader skips a glyph mapped to nothing. The changed lines are
marked `// Kron:`.

The latest release, 0.1.1, has the same code. Delete this folder and the
`[patch.crates-io]` entry once a hayro-cmap release skips such entries.
