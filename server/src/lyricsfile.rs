use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::sync::LazyLock;

pub const LYRICSFILE_VERSION: &str = "1.0";

static TIMESTAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(\d+):(\d{1,2})(?:[\.:](\d{1,3}))?\]")
        .expect("Lyricsfile timestamp regex must be valid")
});
static ID_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([^:\]]+):([^\]]*)\]").expect("Lyricsfile ID tag regex must be valid")
});

#[derive(Debug, Clone)]
pub struct LyricsfileTrackMetadata {
    pub title: String,
    pub album_name: String,
    pub artist_name: String,
    pub duration: f64,
}

impl LyricsfileTrackMetadata {
    pub fn new(title: &str, album_name: &str, artist_name: &str, duration: f64) -> Self {
        Self {
            title: title.trim().to_owned(),
            album_name: album_name.trim().to_owned(),
            artist_name: artist_name.trim().to_owned(),
            duration,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedLyricsfile {
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub instrumental: bool,
    pub has_word_sync: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreparedLyrics {
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub lyricsfile: String,
    pub instrumental: bool,
    pub has_word_sync: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct LyricsfileDocument {
    version: String,
    metadata: LyricsfileMetadata,
    lines: Vec<LyricsfileLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plain: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LyricsfileMetadata {
    title: String,
    artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    instrumental: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct LyricsfileLine {
    text: String,
    start_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_ms: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialLyricsfileDocument {
    #[serde(default, deserialize_with = "null_as_default")]
    metadata: PartialLyricsfileMetadata,
    #[serde(default, deserialize_with = "null_as_default")]
    lines: Vec<PartialLyricsfileLine>,
    plain: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialLyricsfileMetadata {
    #[serde(default)]
    instrumental: bool,
}

#[derive(Debug, Default, Deserialize)]
struct PartialLyricsfileLine {
    text: Option<String>,
    start_ms: Option<i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    words: Vec<PartialLyricsfileWord>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialLyricsfileWord {
    text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimedLine {
    timestamp_ms: i64,
    text: String,
}

fn build_lyricsfile(
    metadata: &LyricsfileTrackMetadata,
    plain_lyrics: Option<&str>,
    synced_lyrics: Option<&str>,
    instrumental: bool,
) -> Result<String> {
    let plain = normalize_non_empty(plain_lyrics);
    let synced = normalize_non_empty(synced_lyrics);

    let timed_lines = if instrumental {
        Vec::new()
    } else {
        synced.as_deref().map(parse_lrc_lines).unwrap_or_default()
    };
    let lines = timed_lines
        .iter()
        .enumerate()
        .map(|(index, line)| LyricsfileLine {
            text: line.text.clone(),
            start_ms: line.timestamp_ms,
            end_ms: timed_lines.get(index + 1).map(|next| next.timestamp_ms),
        })
        .collect();
    let plain = if instrumental {
        None
    } else {
        plain.or_else(|| synced.as_deref().and_then(plain_lyrics_from_lrc))
    };

    let document = LyricsfileDocument {
        version: LYRICSFILE_VERSION.to_owned(),
        metadata: LyricsfileMetadata {
            title: metadata.title.clone(),
            artist: metadata.artist_name.clone(),
            album: normalize_non_empty(Some(&metadata.album_name)),
            duration_ms: duration_to_ms(metadata.duration),
            offset_ms: None,
            language: None,
            instrumental,
        },
        lines,
        plain,
    };

    serde_yaml::to_string(&document).context("failed to serialize Lyricsfile")
}

pub fn prepare_legacy_lyrics(
    metadata: &LyricsfileTrackMetadata,
    plain_lyrics: Option<&str>,
    synced_lyrics: Option<&str>,
    instrumental: bool,
) -> Result<PreparedLyrics> {
    prepare_legacy_lyrics_internal(metadata, plain_lyrics, synced_lyrics, instrumental, true)
}

pub fn prepare_legacy_lyrics_preserving_instrumental(
    metadata: &LyricsfileTrackMetadata,
    plain_lyrics: Option<&str>,
    synced_lyrics: Option<&str>,
    instrumental: bool,
) -> Result<PreparedLyrics> {
    prepare_legacy_lyrics_internal(metadata, plain_lyrics, synced_lyrics, instrumental, false)
}

fn prepare_legacy_lyrics_internal(
    metadata: &LyricsfileTrackMetadata,
    plain_lyrics: Option<&str>,
    synced_lyrics: Option<&str>,
    instrumental: bool,
    detect_instrumental_marker: bool,
) -> Result<PreparedLyrics> {
    let mut plain_lyrics = normalize_non_empty(plain_lyrics);
    let mut synced_lyrics = normalize_non_empty(synced_lyrics);
    let instrumental = instrumental
        || (detect_instrumental_marker
            && synced_lyrics.as_deref().is_some_and(is_instrumental_lrc));
    let lyricsfile = build_lyricsfile(
        metadata,
        plain_lyrics.as_deref(),
        synced_lyrics.as_deref(),
        instrumental,
    )?;

    if instrumental {
        plain_lyrics = None;
        synced_lyrics = None;
    } else if plain_lyrics.is_none() {
        plain_lyrics = synced_lyrics.as_deref().and_then(plain_lyrics_from_lrc);
    }

    Ok(PreparedLyrics {
        plain_lyrics,
        synced_lyrics,
        lyricsfile,
        instrumental,
        // Legacy plain/synced inputs have no way to express word timing.
        has_word_sync: false,
    })
}

pub fn prepare_stored_lyricsfile(lyricsfile: String) -> Result<PreparedLyrics> {
    let parsed = parse_lyricsfile(&lyricsfile)?;

    Ok(PreparedLyrics {
        plain_lyrics: parsed.plain_lyrics,
        synced_lyrics: parsed.synced_lyrics,
        lyricsfile,
        instrumental: parsed.instrumental,
        has_word_sync: parsed.has_word_sync,
    })
}

pub fn parse_lyricsfile(lyricsfile: &str) -> Result<ParsedLyricsfile> {
    let document: PartialLyricsfileDocument =
        serde_yaml::from_str(lyricsfile).context("failed to parse Lyricsfile")?;

    if document.metadata.instrumental {
        return Ok(ParsedLyricsfile {
            instrumental: true,
            ..ParsedLyricsfile::default()
        });
    }

    let has_word_sync = document.lines.iter().any(|line| !line.words.is_empty());

    let synced_lyrics = document
        .lines
        .iter()
        .filter_map(|line| {
            let start_ms = line.start_ms?;
            let text = line_text(line)?;
            Some(format!("{}{}", format_timestamp(start_ms), text))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let synced_lyrics = normalize_non_empty(Some(&synced_lyrics));
    let plain_lyrics = normalize_non_empty(document.plain.as_deref()).or_else(|| {
        let plain = document
            .lines
            .iter()
            .filter_map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        normalize_non_empty(Some(&plain))
    });

    Ok(ParsedLyricsfile {
        plain_lyrics,
        synced_lyrics,
        instrumental: false,
        has_word_sync,
    })
}

/// Best-effort word-sync detection for stored or incoming Lyricsfile text.
///
/// Parse failures are treated as "no word sync"; write paths store malformed
/// Lyricsfiles as-is and already surface the parse error separately.
pub fn lyricsfile_has_word_sync(lyricsfile: &str) -> bool {
    parse_lyricsfile(lyricsfile)
        .map(|parsed| parsed.has_word_sync)
        .unwrap_or(false)
}

pub fn retarget_lyricsfile(
    lyricsfile: &str,
    metadata: &LyricsfileTrackMetadata,
    instrumental: bool,
) -> Result<String> {
    let mut document: Value =
        serde_yaml::from_str(lyricsfile).context("failed to parse existing Lyricsfile")?;
    let root = document
        .as_mapping_mut()
        .context("Lyricsfile root must be a mapping")?;

    let version_key = yaml_key("version");
    if !root.contains_key(&version_key) {
        root.insert(version_key, Value::String(LYRICSFILE_VERSION.to_owned()));
    }

    let metadata_key = yaml_key("metadata");
    if !root.contains_key(&metadata_key) {
        root.insert(metadata_key.clone(), Value::Mapping(Mapping::new()));
    }
    if !root.get(&metadata_key).is_some_and(Value::is_mapping) {
        root.insert(metadata_key.clone(), Value::Mapping(Mapping::new()));
    }

    let effective_instrumental = {
        let metadata_mapping = root
            .get_mut(&metadata_key)
            .and_then(Value::as_mapping_mut)
            .context("Lyricsfile metadata must be a mapping")?;
        let existing_instrumental = metadata_mapping
            .get(yaml_key("instrumental"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let effective_instrumental = instrumental || existing_instrumental;

        metadata_mapping.insert(yaml_key("title"), Value::String(metadata.title.clone()));
        metadata_mapping.insert(
            yaml_key("artist"),
            Value::String(metadata.artist_name.clone()),
        );
        metadata_mapping.insert(
            yaml_key("instrumental"),
            Value::Bool(effective_instrumental),
        );

        if let Some(album) = normalize_non_empty(Some(&metadata.album_name)) {
            metadata_mapping.insert(yaml_key("album"), Value::String(album));
        } else {
            metadata_mapping.remove(yaml_key("album"));
        }

        if let Some(duration_ms) = duration_to_ms(metadata.duration) {
            metadata_mapping.insert(yaml_key("duration_ms"), Value::Number(duration_ms.into()));
        } else {
            metadata_mapping.remove(yaml_key("duration_ms"));
        }

        effective_instrumental
    };

    if effective_instrumental {
        root.insert(yaml_key("lines"), Value::Sequence(Vec::new()));
        root.remove(yaml_key("plain"));
    }

    serde_yaml::to_string(&document).context("failed to serialize retargeted Lyricsfile")
}

fn is_instrumental_lrc(lyrics: &str) -> bool {
    ID_TAG_RE.captures_iter(lyrics).any(|captures| {
        captures[1].trim().eq_ignore_ascii_case("au")
            && captures[2].trim().eq_ignore_ascii_case("instrumental")
    })
}

fn plain_lyrics_from_lrc(synced_lyrics: &str) -> Option<String> {
    let normalized = synced_lyrics.replace("\r\n", "\n");
    let plain = normalized
        .split('\n')
        .filter_map(|line| {
            let trimmed = line.trim();
            let (timestamps, remaining) = parse_timestamp_prefix(trimmed);

            if !timestamps.is_empty() {
                Some(remaining.trim_start().to_owned())
            } else if is_id_tag_line(trimmed) {
                None
            } else {
                Some(line.to_owned())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    normalize_non_empty(Some(&plain))
}

fn is_id_tag_line(line: &str) -> bool {
    ID_TAG_RE
        .find(line)
        .is_some_and(|tag| tag.start() == 0 && tag.end() == line.len())
}

fn normalize_non_empty(value: Option<&str>) -> Option<String> {
    value
        .filter(|content| !content.trim().is_empty())
        .map(str::to_owned)
}

fn parse_lrc_lines(synced_lyrics: &str) -> Vec<TimedLine> {
    let mut lines = synced_lyrics
        .lines()
        .flat_map(|line| {
            let (timestamps, remaining) = parse_timestamp_prefix(line.trim());
            let text = remaining.trim().to_owned();
            timestamps.into_iter().map(move |timestamp_ms| TimedLine {
                timestamp_ms,
                text: text.clone(),
            })
        })
        .collect::<Vec<_>>();

    lines.sort_by_key(|line| line.timestamp_ms);
    lines
}

fn parse_timestamp_prefix(line: &str) -> (Vec<i64>, &str) {
    let mut timestamps = Vec::new();
    let mut remaining = line;

    while let Some(captures) = TIMESTAMP_RE.captures(remaining) {
        let Some(full_match) = captures.get(0) else {
            break;
        };
        let Some(timestamp_ms) = timestamp_from_captures(&captures) else {
            break;
        };

        timestamps.push(timestamp_ms);
        remaining = &remaining[full_match.end()..];
    }

    (timestamps, remaining)
}

fn timestamp_from_captures(captures: &regex::Captures<'_>) -> Option<i64> {
    let minutes = captures.get(1)?.as_str().parse::<i64>().ok()?;
    let seconds = captures.get(2)?.as_str().parse::<i64>().ok()?;
    let milliseconds = captures.get(3).map_or(Some(0), |fraction| {
        let value = fraction.as_str().parse::<i64>().ok()?;
        match fraction.as_str().len() {
            1 => value.checked_mul(100),
            2 => value.checked_mul(10),
            3 => Some(value),
            _ => None,
        }
    })?;

    minutes
        .checked_mul(60_000)?
        .checked_add(seconds.checked_mul(1_000)?)?
        .checked_add(milliseconds)
}

fn line_text(line: &PartialLyricsfileLine) -> Option<String> {
    if !line.words.is_empty() {
        return Some(
            line.words
                .iter()
                .filter_map(|word| word.text.as_deref())
                .collect(),
        );
    }

    line.text.clone()
}

fn format_timestamp(timestamp_ms: i64) -> String {
    let safe_ms = timestamp_ms.max(0);
    let minutes = safe_ms / 60_000;
    let seconds = (safe_ms % 60_000) / 1_000;
    let centiseconds = (safe_ms % 1_000) / 10;

    format!("[{minutes:02}:{seconds:02}.{centiseconds:02}]")
}

fn duration_to_ms(duration: f64) -> Option<i64> {
    if !duration.is_finite() || duration <= 0.0 {
        return None;
    }

    let duration_ms = (duration * 1_000.0).round();
    (duration_ms <= i64::MAX as f64).then_some(duration_ms as i64)
}

fn yaml_key(key: &str) -> Value {
    Value::String(key.to_owned())
}

fn null_as_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> LyricsfileTrackMetadata {
        LyricsfileTrackMetadata::new(" Test Song ", " Test Album ", " Test Artist ", 95.492)
    }

    #[test]
    fn builds_sorted_lines_with_millisecond_precision_and_end_times() {
        let lyricsfile = build_lyricsfile(
            &metadata(),
            None,
            Some("[01:35.492]Later\n[00:03.25]Repeated\n[00:01.5][00:03.25]Repeated"),
            false,
        )
        .unwrap();
        let document: LyricsfileDocument = serde_yaml::from_str(&lyricsfile).unwrap();

        assert_eq!(document.version, "1.0");
        assert_eq!(document.metadata.title, "Test Song");
        assert_eq!(document.metadata.artist, "Test Artist");
        assert_eq!(document.metadata.album.as_deref(), Some("Test Album"));
        assert_eq!(document.metadata.duration_ms, Some(95_492));
        assert_eq!(
            document
                .lines
                .iter()
                .map(|line| (line.start_ms, line.end_ms, line.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1_500, Some(3_250), "Repeated"),
                (3_250, Some(3_250), "Repeated"),
                (3_250, Some(95_492), "Repeated"),
                (95_492, None, "Later"),
            ]
        );
        assert_eq!(document.plain.as_deref(), Some("Later\nRepeated\nRepeated"));
    }

    #[test]
    fn supports_timestamp_without_fraction_and_colon_fraction() {
        let lines = parse_lrc_lines("[0:02]No fraction\n[00:03:4]Colon fraction");

        assert_eq!(
            lines,
            vec![
                TimedLine {
                    timestamp_ms: 2_000,
                    text: "No fraction".to_owned(),
                },
                TimedLine {
                    timestamp_ms: 3_400,
                    text: "Colon fraction".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn plain_derivation_preserves_stanzas_and_untimed_lines() {
        let synced = "[ar:Artist]\r\n[00:01.00]First\r\n\r\n[Verse 2]\r\n[00:02.00]Second\r\n";

        assert_eq!(
            plain_lyrics_from_lrc(synced).as_deref(),
            Some("First\n\n[Verse 2]\nSecond\n")
        );
    }

    #[test]
    fn explicit_instrumental_builds_an_empty_document() {
        let lyricsfile = build_lyricsfile(&metadata(), None, None, true).unwrap();
        let document: LyricsfileDocument = serde_yaml::from_str(&lyricsfile).unwrap();

        assert!(document.metadata.instrumental);
        assert!(document.lines.is_empty());
        assert_eq!(document.plain, None);
    }

    #[test]
    fn legacy_preparation_keeps_representations_consistent() {
        let prepared = prepare_legacy_lyrics(
            &metadata(),
            Some("must be cleared"),
            Some("[AU: instrumental]"),
            false,
        )
        .unwrap();

        assert_eq!(prepared.plain_lyrics, None);
        assert_eq!(prepared.synced_lyrics, None);
        assert!(prepared.instrumental);
        assert!(parse_lyricsfile(&prepared.lyricsfile).unwrap().instrumental);
    }

    #[test]
    fn historical_preparation_preserves_stored_instrumental_state() {
        let prepared = prepare_legacy_lyrics_preserving_instrumental(
            &metadata(),
            None,
            Some("[AU: instrumental]"),
            false,
        )
        .unwrap();

        assert!(!prepared.instrumental);
        assert!(!parse_lyricsfile(&prepared.lyricsfile).unwrap().instrumental);
    }

    #[test]
    fn derives_legacy_text_from_words_and_honors_instrumental() {
        let lyricsfile = r#"
metadata:
  instrumental: false
lines:
  - text: ignored
    start_ms: 12492
    words:
      - text: "Word "
      - text: sync
plain: "  "
"#;

        assert_eq!(
            parse_lyricsfile(lyricsfile).unwrap(),
            ParsedLyricsfile {
                plain_lyrics: Some("Word sync".to_owned()),
                synced_lyrics: Some("[00:12.49]Word sync".to_owned()),
                instrumental: false,
                has_word_sync: true,
            }
        );

        let instrumental = lyricsfile.replace("instrumental: false", "instrumental: true");
        assert_eq!(
            parse_lyricsfile(&instrumental).unwrap(),
            ParsedLyricsfile {
                instrumental: true,
                ..ParsedLyricsfile::default()
            }
        );
    }

    #[test]
    fn detects_word_sync_only_from_non_empty_word_arrays() {
        let word_synced = r#"
metadata:
  instrumental: false
lines:
  - text: Hello world
    start_ms: 1000
    words:
      - text: "Hello "
      - text: world
"#;
        let line_synced = r#"
metadata:
  instrumental: false
lines:
  - text: Hello world
    start_ms: 1000
"#;
        let empty_words = r#"
metadata:
  instrumental: false
lines:
  - text: Hello world
    start_ms: 1000
    words: []
"#;
        let words_only_in_plain = r#"
metadata:
  instrumental: false
lines: []
plain: |
  words: not a word timing array
"#;

        assert!(parse_lyricsfile(word_synced).unwrap().has_word_sync);
        assert!(!parse_lyricsfile(line_synced).unwrap().has_word_sync);
        assert!(!parse_lyricsfile(empty_words).unwrap().has_word_sync);
        assert!(!parse_lyricsfile(words_only_in_plain).unwrap().has_word_sync);
        assert!(lyricsfile_has_word_sync(word_synced));
        assert!(!lyricsfile_has_word_sync("not: [valid"));
    }

    #[test]
    fn legacy_preparation_never_reports_word_sync() {
        let prepared =
            prepare_legacy_lyrics(&metadata(), None, Some("[00:01.00]Hello world"), false).unwrap();

        assert!(!prepared.has_word_sync);
    }

    #[test]
    fn stored_preparation_reports_word_sync() {
        let lyricsfile = r#"
metadata:
  title: Test Song
  artist: Test Artist
  instrumental: false
lines:
  - text: Hello world
    start_ms: 1000
    words:
      - text: "Hello "
        start_ms: 1000
      - text: world
        start_ms: 1500
plain: |
  Hello world
"#;
        let prepared = prepare_stored_lyricsfile(lyricsfile.to_owned()).unwrap();

        assert!(prepared.has_word_sync);
    }

    #[test]
    fn detects_only_the_instrumental_author_tag() {
        assert!(is_instrumental_lrc("[AU: instrumental]"));
        assert!(!is_instrumental_lrc("[ti: instrumental]\n[au: lyrics]"));
    }

    #[test]
    fn retarget_preserves_rich_and_unknown_fields() {
        let original = r#"
version: "1.0"
metadata:
  title: Old
  artist: Old Artist
  album: Old Album
  duration_ms: 1000
  offset_ms: -20
  language: en
  instrumental: false
  provider: local
lines:
  - text: Hello
    start_ms: 10
    words:
      - text: Hello
        start_ms: 10
custom: keep-me
plain: Hello
"#;

        let retargeted = retarget_lyricsfile(original, &metadata(), false).unwrap();
        let document: Value = serde_yaml::from_str(&retargeted).unwrap();

        assert_eq!(document["metadata"]["title"].as_str(), Some("Test Song"));
        assert_eq!(document["metadata"]["duration_ms"].as_i64(), Some(95_492));
        assert_eq!(document["metadata"]["offset_ms"].as_i64(), Some(-20));
        assert_eq!(document["metadata"]["provider"].as_str(), Some("local"));
        assert_eq!(
            document["lines"][0]["words"][0]["text"].as_str(),
            Some("Hello")
        );
        assert_eq!(document["custom"].as_str(), Some("keep-me"));
    }
}
