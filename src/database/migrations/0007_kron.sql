-- A line the meaning check finds says more than its facts is left out as
-- `says_more`, with the words that do. SQLite can't change a CHECK, so the
-- table is rebuilt; nothing references it.
CREATE TABLE unsupported_claims_new (
    id              BLOB PRIMARY KEY,
    generation_id   BLOB NOT NULL REFERENCES generations(id),
    sort_key        TEXT NOT NULL,
    text            TEXT NOT NULL CHECK (length(text) <= 4096),
    reason          TEXT NOT NULL CHECK (reason IN (
                        'invented_figure', 'invented_year', 'no_supporting_fact', 'uses_hidden_fact',
                        'other_entry', 'dropped_qualifier', 'added_qualifier', 'job_echo', 'says_more')),
    figure          TEXT,
    year            INTEGER,
    hidden_fact_id  BLOB REFERENCES facts(id),
    -- The qualifier dropped or added, the word from the job's ad, or the words
    -- the meaning check found say more.
    word            TEXT,
    added_as        BLOB REFERENCES facts(id),  -- the fact the user added from it
    deleted_at      INTEGER,
    CHECK ((reason = 'invented_figure') = (figure IS NOT NULL)),
    CHECK ((reason = 'invented_year') = (year IS NOT NULL)),
    CHECK ((reason = 'uses_hidden_fact') = (hidden_fact_id IS NOT NULL)),
    CHECK ((reason IN ('dropped_qualifier', 'added_qualifier', 'job_echo', 'says_more'))
           = (word IS NOT NULL))
) STRICT;
INSERT INTO unsupported_claims_new SELECT * FROM unsupported_claims;
DROP TABLE unsupported_claims;
ALTER TABLE unsupported_claims_new RENAME TO unsupported_claims;
CREATE INDEX unsupported_claims_by_generation ON unsupported_claims(generation_id);
