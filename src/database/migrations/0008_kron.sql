-- kron.sqlite migration 8: the words of each fact a line used.

-- As the generator copied them from the fact, for the inspector to show
-- which part of the fact the line rests on. NULL: it gave none, as for a
-- bullet, which is asked only for the ids of its facts.
ALTER TABLE line_citations ADD COLUMN words TEXT CHECK (length(words) <= 4096);
