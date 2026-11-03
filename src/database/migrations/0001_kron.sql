-- kron.sqlite migration 1: the first schema. See ARCHITECTURE.md.

-- Sources and facts --------------------------------------------------------
CREATE TABLE files (
    -- BLAKE3 hash of the content (32 bytes); written in hex, it's the file's
    -- name under files/.
    hash        BLOB PRIMARY KEY CHECK (length(hash) = 32),
    media_type  TEXT NOT NULL,
    size        INTEGER NOT NULL CHECK (size >= 0)
) STRICT;

-- An imported file has a name and a hash; a note the user wrote has neither.
CREATE TABLE documents (
    id          BLOB PRIMARY KEY,
    file_name   TEXT,
    file_hash   BLOB REFERENCES files(hash),
    added_at    INTEGER NOT NULL,
    deleted_at  INTEGER,
    CHECK ((file_name IS NULL) = (file_hash IS NULL))
) STRICT;

-- Apart from `documents` so lists never read it: text past about 4 KB goes to
-- overflow pages, which a read of any later column in the row has to walk.
-- Listing 300 documents of 40 KB read 2,996 pages with the text inline, under
-- 10 without. Read only when a document is opened or facts are extracted.
-- Never changes, so the spans in `fact_sources` stay right: to change a note,
-- the user writes a new one and the old one is deleted.
CREATE TABLE document_texts (
    document_id  BLOB PRIMARY KEY REFERENCES documents(id),
    text         TEXT NOT NULL
) STRICT;

-- Portraits for templates that show one, and for applications that ask for one.
CREATE TABLE photos (
    id          BLOB PRIMARY KEY,
    file_hash   BLOB NOT NULL REFERENCES files(hash),  -- the original, never altered
    label       TEXT NOT NULL CHECK (length(label) <= 256),  -- "Formal", "Passport size"
    width       INTEGER NOT NULL CHECK (width > 0),   -- pixels, upright (EXIF rotation applied):
    height      INTEGER NOT NULL CHECK (height > 0),  -- picks the photo nearest a template's shape
    added_at    INTEGER NOT NULL,
    deleted_at  INTEGER
) STRICT;

-- Sections the user names, like "Volunteering" or "Publications". Their own
-- rows, not a name on each entry, so a rename is one edit and a typo can't
-- split one section into two. Anything without a shape of its own fits as
-- text; a new shape is a migration and a template `interface` bump, since a
-- template has to know how to draw it anyway.
CREATE TABLE custom_sections (
    id          BLOB PRIMARY KEY,
    name        TEXT NOT NULL CHECK (length(name) <= 256),
    added_at    INTEGER NOT NULL,
    deleted_at  INTEGER
) STRICT;

CREATE TABLE entries (
    id                 BLOB PRIMARY KEY,
    kind               TEXT NOT NULL CHECK (kind IN (
                           'profile', 'experience', 'education', 'project', 'certification', 'skills',
                           'custom')),
    custom_section_id  BLOB REFERENCES custom_sections(id),
    deleted_at         INTEGER,
    CHECK ((kind = 'custom') = (custom_section_id IS NOT NULL))
) STRICT;

-- Must keep its rowid (never WITHOUT ROWID): `facts_search` finds facts by it.
CREATE TABLE facts (
    id              BLOB PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN (
                        'name', 'bullet', 'title', 'organization', 'date', 'skill',
                        'contact', 'summary', 'location', 'skill_group')),
    text            TEXT NOT NULL CHECK (length(text) <= 4096),
    extracted       TEXT NOT NULL CHECK (length(extracted) <= 4096),
    entry_id        BLOB REFERENCES entries(id),
    hidden          INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),  -- never used in generated output
    -- Why a person should look at it; NULL: nothing to check. Review is the
    -- exception: verbatim facts and the user's own notes arrive with NULL.
    -- Facts needing review are still used in generation, with a warning.
    -- Columns, not a table of their own: the fact list reads them with every
    -- fact, and a separate table measured about 30% slower for it.
    review          TEXT CHECK (review IN ('cleaned', 'unsure', 'structure', 'duplicate')),
    review_note     TEXT CHECK (length(review_note) <= 1024),  -- the details, like "hyphenated across a line break"
    merged_into     BLOB REFERENCES facts(id) CHECK (merged_into != id),
    deleted_at      INTEGER
) STRICT;
CREATE INDEX facts_by_entry ON facts(entry_id);
CREATE INDEX facts_to_review ON facts(entry_id) WHERE review IS NOT NULL;

CREATE TABLE fact_sources (
    id             BLOB PRIMARY KEY,
    fact_id        BLOB NOT NULL REFERENCES facts(id),
    document_id    BLOB NOT NULL REFERENCES documents(id),
    span_start     INTEGER NOT NULL,   -- byte offsets into document_texts.text
    span_end       INTEGER NOT NULL,
    original_text  TEXT NOT NULL,
    deleted_at     INTEGER,
    CHECK (span_start >= 0 AND span_end >= span_start)
) STRICT;
CREATE INDEX fact_sources_by_fact ON fact_sources(fact_id);

-- Full-text search over facts' current text. It indexes `facts` by rowid
-- without copying the text, and the triggers keep it in step. VACUUM can
-- renumber rowids of a table without an INTEGER PRIMARY KEY, so rebuild it
-- after one: INSERT INTO facts_search(facts_search) VALUES ('rebuild').
-- Merged and deleted facts are filtered out by joining back to `facts`.
CREATE VIRTUAL TABLE facts_search USING fts5(text, content = 'facts', content_rowid = 'rowid');

CREATE TRIGGER facts_search_insert AFTER INSERT ON facts BEGIN
    INSERT INTO facts_search (rowid, text) VALUES (new.rowid, new.text);
END;

CREATE TRIGGER facts_search_update AFTER UPDATE OF text ON facts BEGIN
    INSERT INTO facts_search (facts_search, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO facts_search (rowid, text) VALUES (new.rowid, new.text);
END;

CREATE TRIGGER facts_search_delete AFTER DELETE ON facts BEGIN
    INSERT INTO facts_search (facts_search, rowid, text) VALUES ('delete', old.rowid, old.text);
END;

-- Jobs ------------------------------------------------------------------------
CREATE TABLE jobs (
    id               BLOB PRIMARY KEY,
    title            TEXT NOT NULL CHECK (length(title) <= 256),
    company          TEXT NOT NULL CHECK (length(company) <= 256),
    url              TEXT CHECK (length(url) <= 2048),
    created_at       INTEGER NOT NULL,
    board_sort_key   TEXT NOT NULL,    -- order within its column; the column is the stage
    deleted_at       INTEGER
) STRICT;

-- Apart from `jobs` for the same reason as `document_texts`: even as the last
-- column, SQLite keeps up to about 4 KB of it in the row, so a board of 300
-- jobs read 155 pages instead of under 10. Only the job page reads it.
CREATE TABLE job_descriptions (
    job_id  BLOB PRIMARY KEY REFERENCES jobs(id),
    text    TEXT NOT NULL CHECK (length(text) <= 20480)
) STRICT;

CREATE TABLE fact_overrides (
    job_id      BLOB NOT NULL REFERENCES jobs(id),
    fact_id     BLOB NOT NULL REFERENCES facts(id),
    inclusion   TEXT CHECK (inclusion IN ('include', 'exclude')),
    text        TEXT CHECK (length(text) <= 4096),
    -- The key is the job and fact, so overriding the fact again revives
    -- this row: it clears `deleted_at` instead of inserting.
    deleted_at  INTEGER,
    PRIMARY KEY (job_id, fact_id),
    CHECK (inclusion IS NOT NULL OR text IS NOT NULL)
) STRICT;

-- A job's stage, and so its board column, is its latest step event (every kind
-- but `note`); nothing stores it. Adding a job logs `wishlisted`. The first
-- three are the user's plan: moving back between them logs another event,
-- while moving back from a later one deletes that event, since it happened.
CREATE TABLE events (
    id               BLOB PRIMARY KEY,  -- UUIDv7 order is "logged last" for ties
    job_id           BLOB NOT NULL REFERENCES jobs(id),
    at               INTEGER NOT NULL,
    kind             TEXT NOT NULL CHECK (kind IN (
                         'wishlisted', 'tailoring', 'ready',
                         'applied', 'recruiter_replied', 'interview', 'offer',
                         'rejected', 'withdrawn', 'no_response', 'note')),
    -- applied: the versions sent. Each must be a generation of its kind,
    -- checked by the code that logs the event: a CHECK can't see that kind.
    resume_id        BLOB REFERENCES generations(id),
    cover_letter_id  BLOB REFERENCES generations(id),
    interview_kind   TEXT CHECK (interview_kind IN ('phone', 'technical', 'onsite', 'other')),
    interview_with   TEXT CHECK (length(interview_with) <= 512),
    note             TEXT CHECK (length(note) <= 4096),
    deleted_at       INTEGER,
    CHECK ((kind = 'interview') = (interview_kind IS NOT NULL)),
    CHECK ((kind = 'interview') = (interview_with IS NOT NULL)),
    -- Optional even then: applying without one generated here, or without a letter.
    CHECK (resume_id IS NULL OR kind = 'applied'),
    CHECK (cover_letter_id IS NULL OR kind = 'applied')
) STRICT;
CREATE INDEX events_by_job ON events(job_id);

-- Each job's latest step event. Rust maps its kind to a `Stage`, so a new
-- kind fails to compile where it isn't handled, instead of falling through
-- a SQL CASE.
CREATE VIEW latest_steps AS
SELECT job_id, id AS event_id, kind, at
FROM (
    SELECT job_id, id, kind, at,
           ROW_NUMBER() OVER (PARTITION BY job_id ORDER BY at DESC, id DESC) AS position
    FROM events
    WHERE kind != 'note' AND deleted_at IS NULL
)
WHERE position = 1;

-- Templates -------------------------------------------------------------------
-- Designs that lay generations out: Typst source defining `render(resume)`,
-- plus any fonts and images it uses.
CREATE TABLE templates (
    id          BLOB PRIMARY KEY,      -- catalog templates: the catalog's id, the same on every device
    name        TEXT NOT NULL CHECK (length(name) <= 256),
    -- catalog: ours, downloaded (on install, or bought; price and entitlement
    -- are the store server's, not stored here). user: imported by the user.
    origin      TEXT NOT NULL CHECK (origin IN ('catalog', 'user')),
    added_at    INTEGER NOT NULL,
    deleted_at  INTEGER
) STRICT;

-- One release of a template; never changes, so a generation laid out with it
-- keeps its look after the template is updated. The latest is current.
CREATE TABLE template_versions (
    id            BLOB PRIMARY KEY,
    template_id   BLOB NOT NULL REFERENCES templates(id),
    version       INTEGER NOT NULL CHECK (version > 0),
    -- The shape of `resume` it expects, so the app knows which templates an
    -- update of that shape breaks.
    interface     INTEGER NOT NULL CHECK (interface > 0),
    -- The photo's shape it lays out, to pick the nearest photo; NULL: none.
    photo_width   INTEGER CHECK (photo_width > 0),
    photo_height  INTEGER CHECK (photo_height > 0),
    added_at      INTEGER NOT NULL,
    UNIQUE (template_id, version),
    CHECK ((photo_width IS NULL) = (photo_height IS NULL))
) STRICT;

-- A version's files by path; its entry point is `main.typ`.
CREATE TABLE template_files (
    template_version_id  BLOB NOT NULL REFERENCES template_versions(id),
    path                 TEXT NOT NULL,     -- "main.typ", "fonts/Inter.ttf"
    file_hash            BLOB NOT NULL REFERENCES files(hash),
    PRIMARY KEY (template_version_id, path)
) STRICT;

-- What a version can lay out.
CREATE TABLE template_kinds (
    template_version_id  BLOB NOT NULL REFERENCES template_versions(id),
    kind                 TEXT NOT NULL CHECK (kind IN ('resume', 'cover_letter')),
    PRIMARY KEY (template_version_id, kind)
) STRICT;

-- Generated for a job -----------------------------------------------------------
-- Résumés and cover letters are the same kind of data: versioned per job, a
-- tree of sections, blocks and lines that cite facts. A traditional CV, when
-- it comes, is one more kind.
--
-- The template version and photo are kept, so a sent version keeps its look.
-- A cover letter without a template is plain text, for an application form
-- or an email; with one it can also be exported as a PDF.
CREATE TABLE generations (
    id                   BLOB PRIMARY KEY,
    job_id               BLOB NOT NULL REFERENCES jobs(id),
    kind                 TEXT NOT NULL CHECK (kind IN ('resume', 'cover_letter')),
    generated_at         INTEGER NOT NULL,  -- the latest of its kind is current
    template_version_id  BLOB REFERENCES template_versions(id),
    photo_id             BLOB REFERENCES photos(id),  -- NULL: no photo
    -- Per generation, not a setting: applications to different countries
    -- want different sizes. Only a laid-out generation has one.
    paper                TEXT CHECK (paper IN ('a4', 'letter')),
    -- The user's own steer, like "keep it warm, lead with the open-source
    -- work"; sent with the facts. A regeneration starts from the last one's.
    -- Capped above ChatGPT's custom-instruction length (free tier, 1,500).
    instructions         TEXT CHECK (length(instructions) BETWEEN 1 AND 2048),  -- NULL: none
    deleted_at           INTEGER,
    CHECK (kind = 'cover_letter' OR template_version_id IS NOT NULL),
    CHECK ((template_version_id IS NULL) = (paper IS NULL)),
    CHECK (photo_id IS NULL OR template_version_id IS NOT NULL)
) STRICT;
CREATE INDEX generations_by_job ON generations(job_id, kind, generated_at);

-- Each job's current résumé and cover letter.
CREATE VIEW current_generations AS
SELECT job_id, kind, id AS generation_id, generated_at, template_version_id, photo_id, paper
FROM (
    SELECT job_id, kind, id, generated_at, template_version_id, photo_id, paper,
           ROW_NUMBER() OVER (PARTITION BY job_id, kind ORDER BY generated_at DESC, id DESC) AS position
    FROM generations
    WHERE deleted_at IS NULL
)
WHERE position = 1;

-- Templates lay sections out by kind. A cover letter is a header and a body.
CREATE TABLE sections (
    id             BLOB PRIMARY KEY,
    generation_id  BLOB NOT NULL REFERENCES generations(id),
    sort_key       TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN (
                       'header', 'summary', 'experience', 'education', 'projects', 'skills', 'custom',
                       'body')),
    title          TEXT NOT NULL CHECK (length(title) <= 256),  -- empty where none is shown, like the header; a custom one's name
    deleted_at     INTEGER
) STRICT;

CREATE TABLE blocks (
    id          BLOB PRIMARY KEY,
    section_id  BLOB NOT NULL REFERENCES sections(id),
    sort_key    TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN (
                    'header', 'entry', 'skills', 'text', 'item', 'paragraph')),
    deleted_at  INTEGER
) STRICT;

-- One piece of text on the page. Its role is its place in the block:
--   header: name (one), contact
--   entry: title (one); organization, dates, location, stack (one each at most); bullet
--   skills: label (one at most), items (one)
--   text, item: text, item (one)
--   paragraph: sentence
-- Which roles fit which block is checked by the code that writes lines: a
-- CHECK can't see the block's kind.
CREATE TABLE lines (
    id          BLOB PRIMARY KEY,
    block_id    BLOB NOT NULL REFERENCES blocks(id),
    role        TEXT NOT NULL CHECK (role IN (
                    'name', 'contact', 'title', 'organization', 'dates', 'location', 'stack',
                    'bullet', 'label', 'items', 'text', 'item', 'sentence')),
    sort_key    TEXT NOT NULL,
    generated   TEXT NOT NULL CHECK (length(generated) <= 4096),  -- what the generator wrote, to revert to
    text        TEXT NOT NULL CHECK (length(text) <= 4096),  -- what's on the page
    deleted_at  INTEGER
) STRICT;

CREATE TABLE line_citations (
    line_id   BLOB NOT NULL REFERENCES lines(id),
    fact_id   BLOB NOT NULL REFERENCES facts(id),
    sort_key  TEXT NOT NULL,
    PRIMARY KEY (line_id, fact_id)
) STRICT;

-- The facts a generation was made from, as they were then; never changes.
CREATE TABLE fact_versions (
    generation_id  BLOB NOT NULL REFERENCES generations(id),
    fact_id        BLOB NOT NULL REFERENCES facts(id),
    text           TEXT NOT NULL,
    hidden         INTEGER NOT NULL CHECK (hidden IN (0, 1)),
    PRIMARY KEY (generation_id, fact_id)
) STRICT;

-- Claims the generator wanted to make but couldn't back with a fact, so left
-- out. The user can add one as a fact and generate again.
CREATE TABLE unsupported_claims (
    id              BLOB PRIMARY KEY,
    generation_id   BLOB NOT NULL REFERENCES generations(id),
    sort_key        TEXT NOT NULL,
    text            TEXT NOT NULL CHECK (length(text) <= 4096),
    reason          TEXT NOT NULL CHECK (reason IN (
                        'invented_figure', 'invented_year', 'no_supporting_fact', 'uses_hidden_fact')),
    figure          TEXT,
    year            INTEGER,
    hidden_fact_id  BLOB REFERENCES facts(id),
    added_as        BLOB REFERENCES facts(id),  -- the fact the user added from it
    deleted_at      INTEGER,
    CHECK ((reason = 'invented_figure') = (figure IS NOT NULL)),
    CHECK ((reason = 'invented_year') = (year IS NOT NULL)),
    CHECK ((reason = 'uses_hidden_fact') = (hidden_fact_id IS NOT NULL))
) STRICT;

-- LLM usage -------------------------------------------------------------------
-- One row per task run, summing its calls as rig's UsageTotals does. Synced, so
-- spend adds up across devices. Holds counts, never prompts or responses.
-- Never edited or deleted, so no `deleted_at`: a deleted job's runs still cost.
-- A run is measured one of two ways: a subscription run in the credits Kron's
-- server charged, any other in tokens and cost.
CREATE TABLE llm_runs (
    id                    BLOB PRIMARY KEY,
    task                  TEXT NOT NULL CHECK (task IN (
                              'extract_facts', 'generate_resume', 'generate_cover_letter')),
    started_at            INTEGER NOT NULL,
    duration              INTEGER NOT NULL CHECK (duration >= 0),       -- ms
    outcome               TEXT NOT NULL CHECK (outcome IN ('done', 'failed', 'cancelled')),
    -- As they were at the time: providers are per device, so not a reference.
    provider              TEXT NOT NULL,
    vendor                TEXT NOT NULL,
    model                 TEXT NOT NULL,       -- as the response names it
    calls                 INTEGER NOT NULL CHECK (calls >= 0),
    credits               INTEGER CHECK (credits >= 0),
    -- Counters a provider didn't report add nothing, as in rig.
    input_tokens          INTEGER CHECK (input_tokens >= 0),   -- cache reads and writes included
    cached_input_tokens   INTEGER CHECK (cached_input_tokens >= 0),
    cache_written_tokens  INTEGER CHECK (cache_written_tokens >= 0),
    output_tokens         INTEGER CHECK (output_tokens >= 0),  -- reasoning included
    reasoning_tokens      INTEGER CHECK (reasoning_tokens >= 0),
    -- USD, priced when the run ended, since prices change. A lower bound when
    -- some calls weren't priced: local models, or a model rig has no price for.
    cost                  REAL CHECK (cost >= 0),
    unpriced_calls        INTEGER CHECK (unpriced_calls BETWEEN 0 AND calls),
    -- What it ran on. A generation only when one was saved.
    document_id           BLOB REFERENCES documents(id),
    job_id                BLOB REFERENCES jobs(id),
    generation_id         BLOB REFERENCES generations(id),
    CHECK ((task = 'extract_facts') = (document_id IS NOT NULL)),
    CHECK ((task = 'extract_facts') = (job_id IS NULL)),
    CHECK (generation_id IS NULL OR task IN ('generate_resume', 'generate_cover_letter')),
    -- Credits, or every token and cost column: never both, never part.
    CHECK ((credits IS NULL) = (input_tokens IS NOT NULL AND cached_input_tokens IS NOT NULL
        AND cache_written_tokens IS NOT NULL AND output_tokens IS NOT NULL
        AND reasoning_tokens IS NOT NULL AND cost IS NOT NULL AND unpriced_calls IS NOT NULL)),
    CHECK (credits IS NULL OR coalesce(input_tokens, cached_input_tokens, cache_written_tokens,
        output_tokens, reasoning_tokens, cost, unpriced_calls) IS NULL)
) STRICT;

-- Settings --------------------------------------------------------------------
-- One row, columns rather than keys and values, so each setting gets its
-- CHECK and reference; a new one is a migration. Every device creates the row
-- in its first migration, so sync only ever sends its fields' edits. LLM
-- providers are per device, in device.sqlite.
CREATE TABLE settings (
    singleton         INTEGER PRIMARY KEY CHECK (singleton = 1),
    appearance        TEXT NOT NULL DEFAULT 'system' CHECK (appearance IN ('system', 'light', 'dark')),
    motion            TEXT NOT NULL DEFAULT 'system' CHECK (motion IN ('system', 'reduced', 'full')),
    default_template  BLOB REFERENCES templates(id),  -- its newest version is used; NULL: the bundled one
    default_photo     BLOB REFERENCES photos(id)      -- NULL: none
) STRICT;
INSERT INTO settings (singleton) VALUES (1);

-- Sync outbox -------------------------------------------------------------------
-- Changes this device hasn't sent to the sync server yet. A row is deleted
-- once the server accepts it. The server keeps every field's clock and settles
-- conflicts, so a pull only brings values that won: the device takes each one,
-- unless it has an unsent change to that field, and then the newer clock wins.
-- Written only while sync is on: turning it on uploads the records themselves.
-- Here, not in device.sqlite, so a change and its entry commit together: WAL
-- makes no promise across two files. Measured on a heavy user's data, a version
-- per field kept on every device took 48 MB and 9 ms more per generated résumé;
-- this takes nothing once synced and under 1 ms per résumé.
CREATE TABLE outbox (
    table_name  TEXT NOT NULL,
    row_key     BLOB NOT NULL,      -- the primary key's values joined: an id, two ids, an id and a path, 1 for `settings`
    field       TEXT NOT NULL,      -- '': the whole row, for a new one
    clock       INTEGER NOT NULL,   -- hybrid clock value of the change
    PRIMARY KEY (table_name, row_key, field)
) STRICT, WITHOUT ROWID;
