ALTER TABLE lyrics ADD COLUMN lyricsfile TEXT;
ALTER TABLE lyrics ADD COLUMN has_lyricsfile BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX idx_lyrics_has_lyricsfile ON lyrics (has_lyricsfile);
