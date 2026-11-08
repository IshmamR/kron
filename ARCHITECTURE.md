# Architecture

How Kron is built, as of November 2026. The README says what it does and how
to run it.

## The idea

Kron keeps **facts** about the user's work, not résumés. Facts are found in
the documents they import. A résumé is generated for one job from those facts,
and every line it has **cites** the facts it came from. A line the verifier
can't back with its facts is left off the page and listed as an **unsupported
claim**. The citations and the claims are what the app is for, so both stay on
the job's main screen.

## Principles

Data-oriented design, after Mike Acton ("Data-Oriented Design and C++",
CppCon 2014) and Richard Fabian (*Data-Oriented Design*):

- **Know the data.** Design for its real size and how it's read: hundreds of
  jobs, a few thousand facts and events, read far more often than written.
- **Where there's one, there are many.** Work on sets: the grid gets every
  job's stage in one query over `events`, not one lookup per card.
- **Solve the common case first.** That's the screen in front of the user, so
  that's what gets loaded.
- **Compute or precompute by measured cost.** A job's stage, nudges and
  "changed since generated" are cheap, so they're computed when read;
  full-text search isn't, so `facts_search` (FTS5) is kept, and can be
  rebuilt from `facts`.
- **State is which rows exist** where that fits: a job's stage is its latest
  step event, not a field to keep in step.

## The pipeline

```
 file ─► import ─► redaction ─► extraction ─► facts ─┐
 (PDF, Markdown,   (name and    (a model quotes      │
  text, pasted)     contacts     each fact; Kron     ├─► generation ─► verifier ─► lines ─► typeset ─► preview / PDF
                    hidden)      finds the quote)    │   (a model picks          (no model)   (Typst)
                                                     │    and rewords)
 job description, pasted ────────────────────────────┘
```

1. **Import** (`src/import.rs`, `import/pdf.rs`, `import/markdown.rs`) reads a
   file into plain text on the device, with no model. The PDF reader uses
   hayro to collect glyphs, then groups them into words, runs, lines and
   blocks with pdfminer.six's margins. It orders them by XY-cut (Nagy and
   Seth, 1984): side-by-side text is split into columns, unless it is a table
   (one side's lines nearly all have a partner on the other) whose rows wrap
   in step. Icons, vertical text and fake-bold duplicates are dropped, and
   link annotations are attached to their line. `import/name.rs` guesses the
   user's name from font sizes and the email address. The user checks and
   can correct the text before it's saved.
2. **Redaction** (`model/redaction.rs`) runs before anything goes to a model.
   Every way of writing the user's name becomes one made-up name (Settings
   sets it; "Jordan Ellis" by default). Emails, phone numbers, links and
   usernames made from the name become numbered placeholders (`[EMAIL_1]`).
   The user sees what will be hidden and can keep any of it or hide more
   (`workspace/redaction_check.rs`). Company and school names are sent as
   written.
3. **Extraction** (`model/extraction.rs`) sends the redacted text and asks
   for entries (jobs, schools, projects, skill groups) and facts, each as a
   quote. Kron looks each quote up in the text itself, so a fact cites
   exactly where it came from. A quote not found word for word is kept and
   marked for review; a fix the model made is checked by `extraction/fixes.rs`
   (spacing, dashes and split words only, never a number or a word). Contacts
   come back as placeholders and are restored locally; anything on the same
   line as a contact counts as one.
4. **Generation** (`model/generation.rs`) sends the facts, numbered and
   grouped by entry (with no name or contacts), with the job description. The
   model picks entries and skills, and writes the summary and bullets, citing
   facts by number. It may reword a fact and combine two or three. Kron
   writes everything else from the facts as stored: the header, titles,
   organizations, dates and locations. A new résumé starts on the paper last
   chosen, else the locale's (US Letter in the US and a few others, A4
   elsewhere).
5. **The verifier** (`model/verifier.rs`) checks every line the model wrote,
   with no model, so the same line always gets the same answer. See below.
6. **Typesetting** (`typeset.rs`) lays the résumé out with Typst, the one
   layout engine: the in-app preview and the exported PDF are the same pages.
   Templates are Typst files in `assets/typst/` (Modern, Classic, Sidebar);
   each gets the résumé as data, with every line labelled by its id, so a
   click on the page finds the line it hit.

The user can edit any line afterwards (in the inspector). An edit stays on
the page even if the verifier would reject it; the inspector then says what
the line states that its facts don't, and Revert brings back what was
generated.

## The verifier

A line passes only if, in order:

1. it cites at least one fact, and only facts it was given;
2. no cited fact is hidden;
3. a cited fact shares a content word with it (by Snowball stem, NLTK stop
   words left out), or it doesn't count as support;
4. every figure in it is in a supporting fact, compared by value and unit
   (`text2num`: "1.2M" is "1,200,000", "four" is 4, 40% isn't 40); four-digit
   numbers from 1900 to 2099 are years;
5. the words after a count appear in a fact with that count, so "12
   engineers" fails against "12 services" (phrases split RAKE-style).

The summary may also say how many years the person has worked ("5+ years",
"five years") when the experience entries' dates cover that many, overlaps
counted once and a year without a month counted short; it then cites those
dates.

A pass means no invented figure, year or count, not that the line is true. It
misses overstated roles ("contributed" to "led"), added terms ("on AWS"),
negation, months, and merged facts that are each true but false together. It
also flags some true rewordings ("2019–21", "doubled" for "2x"). The inspector
marks the words a line has that its facts don't (`verifier::new_words`), for
the user to check. A model-based meaning check that can only add rejections is
planned, not built.

`model/verifier/cases.txt` holds labelled test lines, and `generated.txt`
lines a model really wrote, labelled by hand; tests fail if a line that must
pass is flagged.

## Storage

SQLite is the source of truth, and each view loads what it shows.

- **Two databases** in the platform's data directory (`kron/`, or `kron-dev/`
  for debug builds): `kron.sqlite` holds the user's records, and
  `device.sqlite` this install's identity, settings and AI providers. Imported
  files live in `files/`, named by their BLAKE3 hash.
- **`Database`** (`database.rs`) is the only state shared between views. It
  owns the connections (WAL, foreign keys on), reads on the main thread, and
  runs writes one at a time on a writer thread, each as a command in one
  transaction. It's passed down through constructors, not a global, so tests
  build their own in memory.
- **The schema is generated from the migrations.** `build.rs` runs
  `src/database/migrations/` on an in-memory SQLite and writes a type per
  table and a typed constant per column, so `write.set(facts::TEXT, id,
  &text)` is checked at compile time: an unknown column, a wrong type, NULL
  into NOT NULL or deleting from a table without `deleted_at` won't build.
  Reads are plain SQL. Migrations run by number (`PRAGMA user_version`).
- **Commands** (`model/commands.rs`, plus `save` in extraction and
  generation) are the only writers. `Write` (`database/write.rs`) records
  what each one changed and the values it replaced.
- **Views refresh from what changed.** After a commit, `Database` emits
  `Changed` with the tables and rows written; each view re-runs its query if
  that touches what it shows. Render never queries.
- **Undo and redo** (`database/write/history.rs`) are two rings of the last
  100 commands, in memory: undo writes the old values back as a new command.
  Settings and device data aren't undone.
- **Kept for sync later**, though sync isn't built: UUIDv7 ids, soft deletes
  (`deleted_at`), fractional sort keys, merged facts pointing at the one kept
  (`merged_into`) instead of being removed, files named by hash, and an
  outbox of changed fields with a hybrid logical clock, written only while
  sync is on.
- **Text caps.** Text people or models write has a CHECK on its length in
  characters (4,096 for facts and lines, 20,480 for a job description), so
  every request to a model has a known worst case and a row a known size.
- **No edit history**, but facts keep `extracted` and lines keep `generated`,
  so either can go back to how it first was. Each generation records its
  template version and paper, and `fact_versions` the facts it used, so a
  résumé whose facts changed since is marked outdated.

## Memory

RAM follows what's on screen. Query results live in the entity of the view
showing them and drop with it; there's no global cache of rows. Rendered pages
are the largest use: `workspace/previews.rs` renders off the main thread,
keeps what views hold plus up to 8 MB of recent renders, and removes dropped
ones from the GPU atlas. Typst's memo cache is trimmed after each render, and
`main` lowers glibc's mmap threshold so freed page buffers go back to the
system.

## AI models

`llm.rs` calls models through rig, on a tokio runtime of its own (GPUI doesn't
run one). Providers, and the model for each task (extract facts, generate
résumé), are per device in `device.sqlite`, because keys and local endpoints
are. Several providers can be set up at once: Anthropic, OpenAI, Gemini,
DeepSeek, xAI, Mistral, Groq and OpenRouter by key, and any OpenAI-compatible
server (Ollama, LM Studio) by URL, whose models are listed from it. The first
provider becomes the default; each task can run on another model, or the
default's.

- API keys are in the OS keyring, never in the database.
- Prompts and responses are never stored or logged. `llm_runs` keeps each
  run's task, tokens and cost.
- The user's name and contacts never reach a model (see Redaction).

## The UI

GPUI through gpui-kit (gpui-component), light and dark themes.

- `main.rs` opens the window and calls each component's `init`.
- `workspace.rs` holds the sidebar and the pages, the undo keys and the
  banner offering "Find facts" or "Set up AI" when a document has no facts
  yet.
- **Résumés** (`workspace/resume_grid.rs`): a card per job with its current
  résumé's thumbnail and what needs attention (unsupported claims, changed
  facts, interviews, follow-ups).
- **A job** (`workspace/job_view.rs`): the résumé's pages, with template and
  paper menus, Regenerate (with optional instructions) and Export PDF; the
  **inspector** (`workspace/inspector.rs`) shows the selected line in an
  editor, the facts it cites with their sources, and the unsupported claims.
  The Application tab (`workspace/application_view.rs`) logs events
  (applied, interviews, offers) from which `model/application.rs` works out
  the stage and nudges.
- **Facts** (`workspace/facts_page.rs`, `fact_list.rs`): search, review,
  edit, hide and merge facts, add notes.
- **Import** (`workspace/import_page.rs`): drop, choose or paste documents,
  check the text, hide details, then find facts.
- **Settings**: appearance, the stand-in name, AI providers and the model
  each task runs on (`workspace/ai_settings.rs`).
- **Onboarding** (`workspace/onboarding.rs`) takes the place of the app until
  there's a document, a job or a name.

## Code conventions

`CLAUDE.md` has them: one file per component, named after its main type, never
`mod.rs`; a fixed order inside a file; errors handled rather than unwrapped;
full-word names; comments for a non-obvious why. Thresholds that are ours, not
from a source, say so where they're defined.

## Tests

`cargo test` runs everything in-process: model and command tests against an
in-memory database seeded with the sample persona (`assets/sample/sample.sql`,
"Alex Rahman"), and UI tests that drive views in headless windows with keys
and clicks. Fixture documents are synthetic: `src/import/pdf/`,
`src/import/markdown/` and `src/model/extraction/` (their `.facts` files are
the facts each should yield). Tests that call a real model are `#[ignore]`d
and use the development data's provider: `cargo test live_extraction --
--ignored --nocapture`, and `live_generation` likewise.
