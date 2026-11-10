use super::{SearchFields, TOKENIZER_NAME};
use std::collections::HashSet;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, Term};

/// Score multiplier for an artist query whose required tokens also appear in
/// the requested order. Optional infix connectors may sit in the gaps.
const ARTIST_PHRASE_BOOST: f32 = 2.0;

/// Connector words that join artist credits. Only occurrences strictly between
/// non-connector tokens are optional; leading, trailing, and all-connector
/// queries stay literal so artists named "Feat", "FT", or "Featuring" work.
const ARTIST_CONNECTORS: &[&str] = &["feat", "ft", "featuring"];

/// How the tokens of one structured parameter are matched against a column.
#[derive(Clone, Copy)]
enum ColumnMatch {
    /// Adjacent tokens, in order (FTS5 `column : "phrase"`).
    Phrase,
    /// Every token, in any order. An exact phrase is only a score boost.
    TokenSet,
}

/// Builds a Tantivy query with the search endpoint's matching semantics:
///
/// - `q` takes precedence and requires every term to appear in at least one of
///   the three indexed columns (FTS5 `MATCH 'a b c'` is an implicit AND).
/// - Otherwise `track_name` and `album_name` are phrases scoped to their
///   columns, while `artist_name` requires every token to appear in the artist
///   column in any order. The artist phrase is boosted, not required, so
///   "Eminem, Rihanna" and "Rihanna, Eminem" match the same record.
/// - Artist connector words (`feat`, `ft`, `featuring`) are optional only when
///   they occur strictly between non-connector tokens. Leading, trailing, and
///   all-connector occurrences remain required, so real artist names such as
///   "FT Island" or "Feat" are not weakened.
///
/// Returns `None` when the input cannot match anything (no usable tokens).
pub(super) fn build_query(
    index: &Index,
    fields: &SearchFields,
    q: Option<&str>,
    track_name: Option<&str>,
    artist_name: Option<&str>,
    album_name: Option<&str>,
) -> Option<Box<dyn Query>> {
    if let Some(q) = q {
        return build_keyword_query(index, fields, q);
    }

    build_structured_query(index, fields, track_name?, artist_name, album_name)
}

fn build_keyword_query(index: &Index, fields: &SearchFields, q: &str) -> Option<Box<dyn Query>> {
    let tokens = tokenize(index, q);
    if tokens.is_empty() {
        return None;
    }

    let term_clauses = tokens
        .iter()
        .map(|token| (Occur::Must, term_in_any_column(fields, token)))
        .collect::<Vec<_>>();

    Some(Box::new(BooleanQuery::new(term_clauses)))
}

fn build_structured_query(
    index: &Index,
    fields: &SearchFields,
    track_name: &str,
    artist_name: Option<&str>,
    album_name: Option<&str>,
) -> Option<Box<dyn Query>> {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(3);

    // Every provided constraint must contribute at least one token; a
    // constraint without tokens can never match, like an empty FTS5 phrase.
    for (field, value, column_match) in [
        (fields.name_lower, Some(track_name), ColumnMatch::Phrase),
        (fields.artist_name_lower, artist_name, ColumnMatch::TokenSet),
        (fields.album_name_lower, album_name, ColumnMatch::Phrase),
    ] {
        let Some(value) = value else { continue };
        let tokens = tokenize(index, value);
        if tokens.is_empty() {
            return None;
        }
        clauses.push((Occur::Must, column_query(field, &tokens, column_match)));
    }

    Some(Box::new(BooleanQuery::new(clauses)))
}

/// A single term that may appear in any of the three indexed columns.
fn term_in_any_column(fields: &SearchFields, token: &str) -> Box<dyn Query> {
    let column_clauses = [
        fields.name_lower,
        fields.artist_name_lower,
        fields.album_name_lower,
    ]
    .into_iter()
    .map(|field| (Occur::Should, column_term(field, token)))
    .collect::<Vec<_>>();

    Box::new(BooleanQuery::new(column_clauses))
}

fn column_phrase(field: Field, tokens: &[String]) -> Box<dyn Query> {
    if tokens.len() == 1 {
        return column_term(field, &tokens[0]);
    }

    let terms = tokens
        .iter()
        .map(|token| Term::from_field_text(field, token))
        .collect::<Vec<_>>();
    Box::new(PhraseQuery::new(terms))
}

fn column_query(field: Field, tokens: &[String], column_match: ColumnMatch) -> Box<dyn Query> {
    match column_match {
        ColumnMatch::Phrase => column_phrase(field, tokens),
        ColumnMatch::TokenSet => {
            let unique = unique_tokens(tokens);
            if unique.len() == 1 {
                // A single-token query is literal, including standalone
                // connector words such as "Feat" or "FT".
                return column_term(field, unique[0]);
            }

            let optional = optional_connector_tokens(tokens);
            let optional_count = tokens
                .iter()
                .filter(|token| optional.contains(token.as_str()))
                .count() as u32;

            let mut clauses = Vec::with_capacity(unique.len() + 1);
            for token in &unique {
                let occur = if optional.contains(token) {
                    Occur::Should
                } else {
                    Occur::Must
                };
                clauses.push((occur, column_term(field, token)));
            }

            // Boost the order of the required tokens, allowing optional
            // infix connectors to occupy the gaps.
            let phrase_tokens = unique
                .iter()
                .copied()
                .filter(|token| !optional.contains(token))
                .collect::<Vec<_>>();
            if phrase_tokens.len() > 1 {
                clauses.push((
                    Occur::Should,
                    Box::new(BoostQuery::new(
                        column_phrase_strs(field, &phrase_tokens, optional_count),
                        ARTIST_PHRASE_BOOST,
                    )),
                ));
            }
            Box::new(BooleanQuery::new(clauses))
        }
    }
}

/// Connector tokens that occur only strictly between non-connector tokens.
/// If a connector also appears at either edge, it remains required.
fn optional_connector_tokens(tokens: &[String]) -> HashSet<&str> {
    let first_non_connector = tokens.iter().position(|token| !is_connector(token));
    let last_non_connector = tokens.iter().rposition(|token| !is_connector(token));

    let mut optional = HashSet::new();
    let mut required = HashSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if !is_connector(token) {
            continue;
        }
        let infix = first_non_connector.is_some_and(|first| index > first)
            && last_non_connector.is_some_and(|last| index < last);
        if infix {
            optional.insert(token.as_str());
        } else {
            required.insert(token.as_str());
        }
    }
    optional.retain(|token| !required.contains(token));
    optional
}

fn is_connector(token: &str) -> bool {
    ARTIST_CONNECTORS.contains(&token)
}

fn column_phrase_strs(field: Field, tokens: &[&str], slop: u32) -> Box<dyn Query> {
    let terms = tokens
        .iter()
        .map(|token| Term::from_field_text(field, token))
        .collect::<Vec<_>>();
    let mut phrase = PhraseQuery::new(terms);
    phrase.set_slop(slop);
    Box::new(phrase)
}

/// Removes duplicate tokens while preserving the order of first occurrence.
fn unique_tokens(tokens: &[String]) -> Vec<&str> {
    let mut seen = HashSet::with_capacity(tokens.len());
    let mut unique = Vec::with_capacity(tokens.len());
    for token in tokens {
        if seen.insert(token.as_str()) {
            unique.push(token.as_str());
        }
    }
    unique
}

fn column_term(field: Field, token: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, token),
        IndexRecordOption::WithFreqs,
    ))
}

fn tokenize(index: &Index, text: &str) -> Vec<String> {
    let mut analyzer = index
        .tokenizers()
        .get(TOKENIZER_NAME)
        .expect("search index tokenizer is registered at open time");

    let mut tokens = Vec::new();
    let mut stream = analyzer.token_stream(text);
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }

    tokens
}
