-- device.sqlite migration 3: text size.

-- How big text is, and with it everything measured in it: per device, as it
-- depends on the screen and how far away it is.
ALTER TABLE device_settings ADD COLUMN text_size TEXT NOT NULL DEFAULT 'default'
    CHECK (text_size IN ('small', 'default', 'large', 'larger'));
