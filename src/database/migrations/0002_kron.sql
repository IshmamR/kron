-- kron.sqlite migration 2: indexes the facts page needs to stay fast on a
-- heavy user's data. See docs/storage.md.

-- Following merges: from a fact to the facts merged into it. Partial, since
-- few facts are ever merged.
CREATE INDEX facts_by_merged_into ON facts(merged_into) WHERE merged_into IS NOT NULL;

-- The lines that cite a fact, for where it's used.
CREATE INDEX line_citations_by_fact ON line_citations(fact_id);
