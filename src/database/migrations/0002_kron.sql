-- Whether the user has been shown that a résumé's lines open the facts they
-- came from: once they've selected one, or closed the note saying so. Synced,
-- since it's about the person, not the device.
ALTER TABLE settings ADD COLUMN resume_lines_explained INTEGER NOT NULL DEFAULT 0
    CHECK (resume_lines_explained IN (0, 1));
