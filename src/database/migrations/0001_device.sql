-- device.sqlite migration 1: the first schema. See ARCHITECTURE.md.

-- One row: its key can only be 1, so a second insert fails. Written by the
-- app on first open, since SQL can't make a UUIDv7.
CREATE TABLE device (
    singleton     INTEGER PRIMARY KEY CHECK (singleton = 1),
    id            BLOB NOT NULL,
    clock         INTEGER NOT NULL,   -- last hybrid clock value issued: ms << 16 | counter
    pulled_until  INTEGER             -- the sync server's change number pulled up to; NULL: sync off
) STRICT;

-- One row, like `settings`.
CREATE TABLE device_settings (
    singleton          INTEGER PRIMARY KEY CHECK (singleton = 1),
    -- As the window was last closed; NULL: never closed, open at the default.
    window_state       TEXT CHECK (window_state IN ('windowed', 'maximized', 'fullscreen')),
    window_x           INTEGER,
    window_y           INTEGER,
    window_width       INTEGER CHECK (window_width > 0),
    window_height      INTEGER CHECK (window_height > 0),
    sidebar_collapsed  INTEGER NOT NULL DEFAULT 0 CHECK (sidebar_collapsed IN (0, 1)),
    last_page          TEXT NOT NULL DEFAULT 'resumes' CHECK (last_page IN ('resumes', 'job', 'facts', 'settings')),
    last_job           BLOB,           -- the job page's job; not a reference, jobs are in kron.sqlite
    CHECK ((window_state IS NULL) = (window_x IS NULL)),
    CHECK ((window_state IS NULL) = (window_y IS NULL)),
    CHECK ((window_state IS NULL) = (window_width IS NULL)),
    CHECK ((window_state IS NULL) = (window_height IS NULL)),
    CHECK ((last_page = 'job') = (last_job IS NOT NULL))
) STRICT;
INSERT INTO device_settings (singleton) VALUES (1);

-- LLMs ------------------------------------------------------------------------
-- Per device: keys live in this device's keyring, and an endpoint like a local
-- Ollama only exists here. Devices share a setup by pointing at the same Kron
-- server, hosted (the subscription) or self-hosted, which is a provider like
-- any other and keeps its own providers and models.
CREATE TABLE llm_providers (
    id        BLOB PRIMARY KEY,        -- also names its key in the OS keyring
    name      TEXT NOT NULL,           -- "Work OpenAI", "Home server"
    vendor    TEXT NOT NULL,           -- rig's vendor id, or 'kron'; rig owns the list
    format    TEXT,                    -- request format for OpenAI-style servers; NULL: the vendor's
    base_url  TEXT,                    -- NULL: the vendor's own endpoint
    added_at  INTEGER NOT NULL
) STRICT;

-- Which model runs each task. A task without a row uses `default`; with
-- neither, the app asks the user to set one up. Model lists are fetched from
-- the provider, not stored.
CREATE TABLE llm_assignments (
    task         TEXT PRIMARY KEY CHECK (task IN (
                     'default', 'extract_facts', 'generate_resume', 'generate_cover_letter')),
    provider_id  BLOB NOT NULL REFERENCES llm_providers(id) ON DELETE CASCADE,
    model        TEXT NOT NULL
) STRICT, WITHOUT ROWID;
