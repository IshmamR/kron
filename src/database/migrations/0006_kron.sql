-- Graphite is the dark palette by default, in place of Orchid's violet, and
-- takes over where Orchid was still the default. SQLite can't change a
-- column's default, so the table is rebuilt; nothing references it.
CREATE TABLE settings_new (
    singleton         INTEGER PRIMARY KEY CHECK (singleton = 1),
    appearance        TEXT NOT NULL DEFAULT 'system' CHECK (appearance IN ('system', 'light', 'dark')),
    motion            TEXT NOT NULL DEFAULT 'system' CHECK (motion IN ('system', 'reduced', 'full')),
    default_template  BLOB REFERENCES templates(id),  -- its newest version is used; NULL: the bundled one
    default_photo     BLOB REFERENCES photos(id),     -- NULL: none
    -- The made-up name sent to models in place of the user's.
    stand_in_name     TEXT NOT NULL DEFAULT 'Jordan Ellis' CHECK (trim(stand_in_name) <> ''),
    -- Each mode's palette, kept whichever mode is showing.
    light_palette     TEXT NOT NULL DEFAULT 'linen'
                          CHECK (light_palette IN ('porcelain', 'linen', 'glacier', 'clay', 'paper', 'lilac')),
    dark_palette      TEXT NOT NULL DEFAULT 'graphite'
                          CHECK (dark_palette IN ('moss', 'midnight', 'copper', 'orchid', 'olive', 'graphite')),
    resume_lines_explained INTEGER NOT NULL DEFAULT 0 CHECK (resume_lines_explained IN (0, 1))
) STRICT;
INSERT INTO settings_new SELECT * FROM settings;
UPDATE settings_new SET dark_palette = 'graphite' WHERE dark_palette = 'orchid';
DROP TABLE settings;
ALTER TABLE settings_new RENAME TO settings;
