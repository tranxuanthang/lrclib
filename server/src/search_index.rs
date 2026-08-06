mod indexer;
mod query;

pub use indexer::{spawn_indexer, IndexerConfig, IndexerHandle};

use crate::entities::track::TrackSearchDoc;
use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tantivy::collector::TopDocs;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, INDEXED,
};
use tantivy::tokenizer::{LowerCaser, SimpleTokenizer, TextAnalyzer};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument};

/// Maximum number of ranked results returned by a search.
pub const SEARCH_RESULT_LIMIT: usize = 20;

/// Bump this when the schema or tokenizer changes; a mismatch triggers a full
/// rebuild of the index from the `tracks` table.
const INDEX_FORMAT_VERSION: u32 = 1;
const VERSION_FILE_NAME: &str = "lrclib_index_version";

/// Values indexed by this tokenizer are already normalized by
/// `utils::prepare_input` (lowercased, punctuation stripped, diacritics
/// removed), so a plain split-on-non-alphanumeric plus lowercase safety net is
/// sufficient for the search index.
const TOKENIZER_NAME: &str = "lrclib_normalized";

const TRACK_ID_FIELD: &str = "track_id";
const NAME_FIELD: &str = "name_lower";
const ARTIST_FIELD: &str = "artist_name_lower";
const ALBUM_FIELD: &str = "album_name_lower";

#[derive(Clone, Copy)]
pub(crate) struct SearchFields {
    track_id: Field,
    name_lower: Field,
    artist_name_lower: Field,
    album_name_lower: Field,
}

impl SearchFields {
    fn from_schema(schema: &Schema) -> Result<Self> {
        Ok(Self {
            track_id: schema.get_field(TRACK_ID_FIELD)?,
            name_lower: schema.get_field(NAME_FIELD)?,
            artist_name_lower: schema.get_field(ARTIST_FIELD)?,
            album_name_lower: schema.get_field(ALBUM_FIELD)?,
        })
    }
}

/// Embedded Tantivy index over track metadata.
///
/// SQLite remains the source of truth: this index only stores normalized track
/// names plus the track id, and can always be rebuilt from the `tracks` table.
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    fields: SearchFields,
    ready: AtomicBool,
}

impl SearchIndex {
    /// Opens the index at `dir`, recreating it from scratch when it is
    /// missing, incompatible, or unreadable.
    pub fn open_or_create(dir: &Path) -> Result<Self> {
        if stored_version(dir) == Some(INDEX_FORMAT_VERSION) {
            match Self::open(dir) {
                Ok(search_index) => return Ok(search_index),
                Err(error) => {
                    tracing::warn!(
                        directory = %dir.display(),
                        error = %error,
                        "failed to open existing search index; recreating"
                    );
                }
            }
        }

        Self::recreate(dir)
    }

    fn open(dir: &Path) -> Result<Self> {
        let index = Index::open_in_dir(dir)?;
        Self::from_index(index)
    }

    fn recreate(dir: &Path) -> Result<Self> {
        ensure_safe_to_recreate(dir)?;
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        std::fs::create_dir_all(dir)?;

        let index = Index::create_in_dir(dir, build_schema())?;
        std::fs::write(
            dir.join(VERSION_FILE_NAME),
            INDEX_FORMAT_VERSION.to_string(),
        )?;

        Self::from_index(index)
    }

    fn from_index(index: Index) -> Result<Self> {
        register_tokenizer(&index);
        let fields = SearchFields::from_schema(&index.schema())?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        Ok(Self {
            index,
            reader,
            fields,
            ready: AtomicBool::new(false),
        })
    }

    /// Returns the top ranked track ids for the given normalized search
    /// parameters. Inputs are expected to be pre-processed by
    /// `utils::prepare_input`.
    pub fn search(
        &self,
        q: Option<&str>,
        track_name: Option<&str>,
        artist_name: Option<&str>,
        album_name: Option<&str>,
    ) -> Result<Vec<i64>> {
        // At least q or track_name must be present.
        if q.is_none() && track_name.is_none() {
            return Ok(Vec::new());
        }

        let query = match query::build_query(
            &self.index,
            &self.fields,
            q,
            track_name,
            artist_name,
            album_name,
        ) {
            Some(query) => query,
            None => return Ok(Vec::new()),
        };

        let searcher = self.reader.searcher();
        let top_docs = searcher.search(
            &query,
            &TopDocs::with_limit(SEARCH_RESULT_LIMIT).order_by_score(),
        )?;

        let mut track_ids = Vec::with_capacity(top_docs.len());
        for (_score, doc_address) in top_docs {
            let track_id_column = searcher
                .segment_reader(doc_address.segment_ord)
                .fast_fields()
                .i64(TRACK_ID_FIELD)?;
            if let Some(track_id) = track_id_column.first(doc_address.doc_id) {
                track_ids.push(track_id);
            }
        }

        Ok(track_ids)
    }

    /// Whether the indexer has caught up with the `tracks` table at least once
    /// since startup. Search is unavailable until this returns true.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    /// The id of the last track covered by a committed index generation,
    /// stored as the commit payload so it is atomic with the commit itself.
    pub fn last_indexed_track_id(&self) -> Result<i64> {
        match self.index.load_metas()?.payload {
            Some(payload) => payload
                .parse()
                .with_context(|| format!("invalid search index commit payload: {payload}")),
            None => Ok(0),
        }
    }

    pub(crate) fn index(&self) -> &Index {
        &self.index
    }

    pub(crate) fn fields(&self) -> &SearchFields {
        &self.fields
    }

    #[cfg(test)]
    pub(crate) fn reload_reader(&self) -> tantivy::Result<()> {
        self.reader.reload()
    }

    #[cfg(test)]
    pub(crate) fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

pub fn index_dir_from_env(database: &Path) -> PathBuf {
    match std::env::var("LRCLIB_SEARCH_INDEX_DIR")
        .ok()
        .filter(|value| !value.is_empty())
    {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(format!("{}.tantivy", database.display())),
    }
}

/// Opens the search index and spawns its indexer, using
/// `LRCLIB_SEARCH_INDEX_DIR` when configured.
///
/// Any failure is logged and results in `(None, None)`: non-search routes remain
/// available, but search requests fail until the server restarts successfully.
pub fn bootstrap_from_env(
    database: &Path,
    pool: &Pool<SqliteConnectionManager>,
) -> (Option<Arc<SearchIndex>>, Option<IndexerHandle>) {
    let dir = index_dir_from_env(database);
    match SearchIndex::open_or_create(&dir) {
        Ok(search_index) => {
            let search_index = Arc::new(search_index);
            let indexer = spawn_indexer(
                Arc::clone(&search_index),
                pool.clone(),
                IndexerConfig::default(),
            );
            tracing::info!(directory = %dir.display(), "search index enabled");
            (Some(search_index), Some(indexer))
        }
        Err(error) => {
            tracing::error!(
                directory = %dir.display(),
                error = %error,
                "failed to open search index; search is unavailable"
            );
            (None, None)
        }
    }
}

fn build_schema() -> Schema {
    let mut schema_builder = Schema::builder();

    let text_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );

    schema_builder.add_i64_field(TRACK_ID_FIELD, FAST | INDEXED);
    schema_builder.add_text_field(NAME_FIELD, text_options.clone());
    schema_builder.add_text_field(ARTIST_FIELD, text_options.clone());
    schema_builder.add_text_field(ALBUM_FIELD, text_options);

    schema_builder.build()
}

fn register_tokenizer(index: &Index) {
    let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .build();
    index.tokenizers().register(TOKENIZER_NAME, analyzer);
}

fn stored_version(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join(VERSION_FILE_NAME))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Guards against wiping a directory that was never a search index, for
/// example when `LRCLIB_SEARCH_INDEX_DIR` points somewhere unintended.
fn ensure_safe_to_recreate(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    let is_empty = std::fs::read_dir(dir)?.next().is_none();
    if is_empty || dir.join(VERSION_FILE_NAME).exists() || dir.join("meta.json").exists() {
        return Ok(());
    }

    anyhow::bail!(
        "refusing to recreate search index at {} because the directory does not look like a search index",
        dir.display()
    )
}

pub(crate) fn build_document(fields: &SearchFields, track: &TrackSearchDoc) -> TantivyDocument {
    let mut document = TantivyDocument::default();
    document.add_i64(fields.track_id, track.id);

    for (field, value) in [
        (fields.name_lower, &track.name_lower),
        (fields.artist_name_lower, &track.artist_name_lower),
        (fields.album_name_lower, &track.album_name_lower),
    ] {
        if let Some(value) = value {
            if !value.is_empty() {
                document.add_text(field, value);
            }
        }
    }

    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("lrclib-search-index-{}", Uuid::new_v4()))
    }

    fn doc(id: i64, name: &str, artist: &str, album: &str) -> TrackSearchDoc {
        TrackSearchDoc {
            id,
            name_lower: Some(name.to_owned()),
            artist_name_lower: Some(artist.to_owned()),
            album_name_lower: Some(album.to_owned()),
        }
    }

    fn seed(search_index: &SearchIndex, docs: &[TrackSearchDoc]) {
        let mut writer = search_index
            .index()
            .writer::<TantivyDocument>(32 * 1024 * 1024)
            .expect("failed to create test writer");
        for track in docs {
            writer
                .add_document(build_document(search_index.fields(), track))
                .expect("failed to add test document");
        }
        writer.commit().expect("failed to commit test documents");
        search_index
            .reload_reader()
            .expect("failed to reload reader");
    }

    fn search(
        search_index: &SearchIndex,
        q: Option<&str>,
        track_name: Option<&str>,
        artist_name: Option<&str>,
        album_name: Option<&str>,
    ) -> Vec<i64> {
        search_index
            .search(q, track_name, artist_name, album_name)
            .expect("search failed")
    }

    #[test]
    fn keyword_search_requires_every_term() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(&search_index, &[doc(1, "benchmark song", "tester", "demo")]);

        assert_eq!(
            search(&search_index, Some("benchmark song"), None, None, None),
            vec![1]
        );
        assert!(search(&search_index, Some("benchmark missing"), None, None, None).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keyword_terms_match_across_columns() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(&search_index, &[doc(7, "benchmark song", "tester", "demo")]);

        assert_eq!(
            search(
                &search_index,
                Some("benchmark tester demo"),
                None,
                None,
                None
            ),
            vec![7]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn structured_search_requires_artist_phrase_in_the_artist() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(
            &search_index,
            &[
                doc(1, "scoped song", "alpha beta", "collection"),
                doc(2, "scoped song", "alpha", "beta collection"),
                doc(3, "scoped song", "alpha featured beta", "collection"),
            ],
        );

        assert_eq!(
            search(
                &search_index,
                None,
                Some("scoped song"),
                Some("alpha beta"),
                None
            ),
            vec![1]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn structured_search_filters_by_album_phrase() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(
            &search_index,
            &[
                doc(1, "scoped song", "alpha beta", "collection"),
                doc(2, "scoped song", "alpha", "beta collection"),
            ],
        );

        assert_eq!(
            search(
                &search_index,
                None,
                Some("scoped song"),
                None,
                Some("beta collection")
            ),
            vec![2]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ranking_prefers_closer_matches() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(
            &search_index,
            &[
                doc(1, "hello there general kenobi", "artist", "album"),
                doc(2, "hello", "artist", "album"),
            ],
        );

        assert_eq!(
            search(&search_index, Some("hello"), None, None, None),
            vec![2, 1]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_or_unusable_inputs_return_no_results() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(&search_index, &[doc(1, "some song", "artist", "album")]);

        assert!(search(&search_index, None, None, None, None).is_empty());
        assert!(search(&search_index, Some("★★★"), None, None, None).is_empty());
        assert!(search(&search_index, None, None, Some("artist"), None).is_empty());
        assert!(search(&search_index, None, Some("some song"), Some("★★★"), None).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalized_unicode_terms_match() {
        let dir = test_dir();
        let search_index = SearchIndex::open_or_create(&dir).unwrap();
        seed(
            &search_index,
            &[
                doc(1, "dont stop", "artist", "album"),
                doc(2, "残酷な天使のテーゼ", "高橋洋子", "album"),
            ],
        );

        assert_eq!(
            search(&search_index, Some("dont stop"), None, None, None),
            vec![1]
        );
        assert_eq!(
            search(&search_index, Some("残酷な天使のテーゼ"), None, None, None),
            vec![2]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_survives_reopen() {
        let dir = test_dir();
        {
            let search_index = SearchIndex::open_or_create(&dir).unwrap();
            seed(
                &search_index,
                &[doc(1, "persisted song", "artist", "album")],
            );
        }

        let reopened = SearchIndex::open_or_create(&dir).unwrap();
        assert_eq!(
            search(&reopened, Some("persisted song"), None, None, None),
            vec![1]
        );
        assert!(!reopened.is_ready());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_mismatch_triggers_rebuild() {
        let dir = test_dir();
        {
            let search_index = SearchIndex::open_or_create(&dir).unwrap();
            seed(&search_index, &[doc(1, "old song", "artist", "album")]);
        }
        std::fs::write(dir.join(VERSION_FILE_NAME), "0").unwrap();

        let recreated = SearchIndex::open_or_create(&dir).unwrap();
        assert_eq!(recreated.num_docs(), 0);
        assert_eq!(stored_version(&dir), Some(INDEX_FORMAT_VERSION));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_recreate_foreign_directory() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("unrelated.txt"), "not a search index").unwrap();

        assert!(SearchIndex::open_or_create(&dir).is_err());
        assert!(dir.join("unrelated.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
