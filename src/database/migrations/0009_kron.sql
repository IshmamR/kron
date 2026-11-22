-- kron.sqlite migration 9: each AI call of a run, on its own.

-- A résumé is written by one call and checked by another, and what each took
-- and cost is shown apart. The run keeps what its calls share; their counts
-- and costs move to llm_calls. SQLite can't drop a column a CHECK names, so
-- llm_runs is rebuilt; nothing references it.
CREATE TABLE llm_runs_new (
    id                    BLOB PRIMARY KEY,
    task                  TEXT NOT NULL CHECK (task IN (
                              'extract_facts', 'generate_resume', 'generate_cover_letter')),
    started_at            INTEGER NOT NULL,
    duration              INTEGER NOT NULL CHECK (duration >= 0),       -- ms
    outcome               TEXT NOT NULL CHECK (outcome IN ('done', 'failed', 'cancelled')),
    -- As they were at the time: providers are per device, so not a reference.
    provider              TEXT NOT NULL,
    vendor                TEXT NOT NULL,
    -- What it ran on. A generation only when one was saved.
    document_id           BLOB REFERENCES documents(id),
    job_id                BLOB REFERENCES jobs(id),
    generation_id         BLOB REFERENCES generations(id),
    CHECK ((task = 'extract_facts') = (document_id IS NOT NULL)),
    CHECK ((task = 'extract_facts') = (job_id IS NULL)),
    CHECK (generation_id IS NULL OR task IN ('generate_resume', 'generate_cover_letter'))
) STRICT;

CREATE TABLE llm_calls (
    id                    BLOB PRIMARY KEY,
    run_id                BLOB NOT NULL REFERENCES llm_runs_new(id),
    -- NULL: the run's calls together, recorded before they were apart.
    step                  TEXT CHECK (step IN ('extract_facts', 'write', 'check_meaning')),
    model                 TEXT NOT NULL,       -- as the response names it
    duration              INTEGER NOT NULL CHECK (duration >= 0),       -- ms
    credits               INTEGER CHECK (credits >= 0),
    -- NULL when no answer came, or it was paid in credits. Counters a
    -- provider didn't report add nothing, as in rig.
    input_tokens          INTEGER CHECK (input_tokens >= 0),   -- cache reads and writes included
    cached_input_tokens   INTEGER CHECK (cached_input_tokens >= 0),
    cache_written_tokens  INTEGER CHECK (cache_written_tokens >= 0),
    output_tokens         INTEGER CHECK (output_tokens >= 0),  -- reasoning included
    reasoning_tokens      INTEGER CHECK (reasoning_tokens >= 0),
    -- USD, priced when the call ended, since prices change. NULL: no answer,
    -- or no price known, as for local models or a ChatGPT plan.
    cost                  REAL CHECK (cost >= 0),
    CHECK ((input_tokens IS NULL) = (output_tokens IS NULL)
        AND (input_tokens IS NULL) = (cached_input_tokens IS NULL)
        AND (input_tokens IS NULL) = (cache_written_tokens IS NULL)
        AND (input_tokens IS NULL) = (reasoning_tokens IS NULL)),
    CHECK (cost IS NULL OR input_tokens IS NOT NULL),
    CHECK (credits IS NULL OR input_tokens IS NULL)
) STRICT;

INSERT INTO llm_runs_new
SELECT id, task, started_at, duration, outcome, provider, vendor, document_id, job_id, generation_id
FROM llm_runs;

-- A run of one call keeps its step; one of more can't be split. A cost that
-- left a call unpriced was a lower bound, so it isn't kept.
INSERT INTO llm_calls
SELECT randomblob(16), id,
       CASE WHEN calls = 1 THEN
           CASE task WHEN 'extract_facts' THEN 'extract_facts' ELSE 'write' END
       END,
       model, duration, credits,
       input_tokens, cached_input_tokens, cache_written_tokens, output_tokens, reasoning_tokens,
       CASE WHEN unpriced_calls = 0 THEN cost END
FROM llm_runs
WHERE calls > 0;

DROP TABLE llm_runs;
ALTER TABLE llm_runs_new RENAME TO llm_runs;
CREATE INDEX llm_calls_by_run ON llm_calls(run_id);
