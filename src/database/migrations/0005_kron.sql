-- Remember color choices independently of the system's appearance mode.
ALTER TABLE settings ADD COLUMN light_palette TEXT NOT NULL DEFAULT 'linen'
    CHECK (light_palette IN ('porcelain', 'linen', 'glacier', 'clay', 'paper', 'lilac'));
ALTER TABLE settings ADD COLUMN dark_palette TEXT NOT NULL DEFAULT 'orchid'
    CHECK (dark_palette IN ('moss', 'midnight', 'copper', 'orchid', 'olive', 'graphite'));
