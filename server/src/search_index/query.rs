use super::{SearchFields, TOKENIZER_NAME};
use tantivy::query::{BooleanQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, Term};

/// Builds a Tantivy query with the search endpoint's matching semantics:
///
/// - `q` takes precedence and requires every term to appear in at least one of
///   the three indexed columns (FTS5 `MATCH 'a b c'` is an implicit AND).
/// - Otherwise `track_name` is a phrase scoped to the name column, optionally
///   AND-ed with artist and album phrases (FTS5 `column : "phrase"`).
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
    for (field, value) in [
        (fields.name_lower, Some(track_name)),
        (fields.artist_name_lower, artist_name),
        (fields.album_name_lower, album_name),
    ] {
        let Some(value) = value else { continue };
        let tokens = tokenize(index, value);
        if tokens.is_empty() {
            return None;
        }
        clauses.push((Occur::Must, column_phrase(field, &tokens)));
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
