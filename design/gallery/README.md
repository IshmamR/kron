# Gallery workflow proposal

The selected Gallery direction, expanded against the current native app on branch `codex/gallery-workflow-design`.

This is a design reference with interactive sample state. It does not replace the Rust/GPUI application. No Rust files, migrations, user databases, credentials, or production services are changed.

## Review

Start in the résumé gallery. Open Ledgerly to inspect its hidden-fact claim and older sent résumé; open Cinder Labs for regeneration instructions and unsupported-claim repair. Select a line to edit it or follow its citations into Career facts. The same fact can be edited, reviewed, hidden, merged, or followed back to its exact résumé line.

Application opens the real sample timeline, job description, posting link and sent-version information. Log, edit or delete an event and use Undo to restore it. Settings contains General and AI pages; the eight proposed theme presets remain available. Import includes multiple-file sequencing, corrected source text, a privacy review and a saved-document reminder.

The host's design controls expose First launch, Empty library, No AI, Generation error, Typesetting error and Import duplicate. These are states of the same design, not alternative layouts.

See [functionality.md](functionality.md) for the complete native-to-design mapping and explicit boundaries.

## Source and build

- `shell.html`: Gallery shell, visual primitives, palette inputs and the unmodified thumbnail capture from the earlier isolated sample app run.
- `sample-data.json`: the bundled `assets/sample/sample.sql` evaluated in an in-memory database against the current migrations. Dates are anchored to November 3, 2026; names and records belong to the fictional development fixture. Includes 83 facts, nine jobs, 268 résumé lines, 321 citations and 31 application events.
- `workflows.js`: local interaction state and sample mutations, including Undo/Redo.
- `workflows.css`: the additional workflow surfaces.
- `build.py`: combines those sources into an inline fragment; no dependency installation is needed.

```sh
python3 design/gallery/build.py
```

The generated `gallery-workflows.html` is an inline fragment. In Codex, render it with the Visualize skill's `scripts/render.py` to create `preview.html`; keep its sandboxed iframe and CSP. The generated preview and captures are ignored to avoid checking in duplicate copies of the embedded assets. A prepared local preview is available during this review.

## Scope of the simulation

Text imports and drag/drop read local text files. PDF parsing uses a clearly labeled sample-text path. AI generation/extraction uses deterministic sample transformations, not a model; PDF writing and keyring operations stop at illustrated outcomes. The line warning demonstrates presentation and is not a replacement for the native verifier. No AI request or keyring write is performed by the prototype.

Host state stores the theme and a brief current-screen selection. Sample edits, imported text, keys, Undo history and the fixture database are not persisted there. Reloading restores the sample records.

Native file pickers, exact Typst pagination, OS keyring failures, window restoration, actual macOS titlebar behavior and full accessibility validation still require the native implementation. The functionality matrix marks these separately; it does not claim native parity from an HTML preview.

## Validation

The audit ran `cargo test --offline workspace:: -- --nocapture`: **110 passed**. Those tests exercise the existing native workspace, import, facts, inspector, application and keyboard flows. The prototype is separately checked through its sandboxed browser preview; results are recorded in `functionality.md`.
