ALTER TABLE lyrics ADD COLUMN has_word_sync BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX idx_lyrics_has_word_sync ON lyrics (has_word_sync) WHERE has_word_sync = TRUE;
