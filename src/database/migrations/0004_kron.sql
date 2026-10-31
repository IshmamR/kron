-- kron.sqlite migration 4: the made-up name sent to models in place of the
-- user's. See docs/storage.md.

ALTER TABLE settings ADD COLUMN
    stand_in_name TEXT NOT NULL DEFAULT 'Jordan Ellis' CHECK (trim(stand_in_name) <> '');
