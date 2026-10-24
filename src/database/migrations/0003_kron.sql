-- kron.sqlite migration 3: a résumé's or cover letter's tree by its parent
-- keys, so reading one doesn't scan every generation's. See docs/storage.md.

CREATE INDEX sections_by_generation ON sections(generation_id);
CREATE INDEX blocks_by_section ON blocks(section_id);
CREATE INDEX lines_by_block ON lines(block_id);
CREATE INDEX unsupported_claims_by_generation ON unsupported_claims(generation_id);
