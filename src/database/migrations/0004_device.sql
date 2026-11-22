-- device.sqlite migration 4: the text size as a percentage, set with a slider.

ALTER TABLE device_settings ADD COLUMN text_scale INTEGER NOT NULL DEFAULT 100
    CHECK (text_scale BETWEEN 85 AND 130 AND text_scale % 5 = 0);
UPDATE device_settings SET text_scale = CASE text_size
    WHEN 'small' THEN 90 WHEN 'large' THEN 115 WHEN 'larger' THEN 125 ELSE 100 END;
ALTER TABLE device_settings DROP COLUMN text_size;
