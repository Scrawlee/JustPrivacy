-- Display name: same letters as the username, but with the capitals the user wants.
-- Changing only the capitals is free; changing the username itself is limited to once per 30 days.
ALTER TABLE users
    ADD COLUMN display_name        TEXT CHECK (display_name IS NULL OR lower(display_name) = username),
    ADD COLUMN username_changed_at TIMESTAMPTZ;
