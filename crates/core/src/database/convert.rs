//! Implementations to convert between Rust types and SQL data

use crate::{
    collection::{ProfileId, RecipeId},
    database::{
        CollectionId, CollectionMetadata, DatabaseError, ProfileFilter,
    },
    http::{
        Exchange, ExchangeSummary, HttpMethod, HttpVersion, RequestId,
        RequestRecord, ResponseRecord,
    },
};
use bytes::Bytes;
use core::str;
use derive_more::Display;
use reqwest::{
    StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use rusqlite::{
    Row, ToSql,
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
};
use serde::{Serialize, de::DeserializeOwned};
use slumber_util::{ResultTraced, paths};
use std::{
    env,
    fmt::Debug,
    ops::Deref,
    path::{Path, PathBuf},
    str::Utf8Error,
    sync::Arc,
};
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use winnow::{
    ModalResult, Parser,
    combinator::{repeat, terminated},
    token::take_while,
};

impl ToSql for CollectionId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for CollectionId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(Self(Uuid::column_result(value)?))
    }
}

impl ToSql for ProfileId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for ProfileId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(String::column_result(value)?.into())
    }
}

impl ToSql for RequestId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for RequestId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(Self(Uuid::column_result(value)?))
    }
}

impl ToSql for RecipeId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.deref().to_sql()
    }
}

impl FromSql for RecipeId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(String::column_result(value)?.into())
    }
}

impl ToSql for HttpVersion {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.to_str().to_sql()
    }
}

impl FromSql for HttpVersion {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value)?.parse().map_err(error_other)
    }
}

impl ToSql for HttpMethod {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.to_str().to_sql()
    }
}

impl FromSql for HttpMethod {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value)?.parse().map_err(error_other)
    }
}

/// Wrapper to serialize paths as strings in the DB. This is flawed because
/// paths aren't guaranteed to be UTF-8 on either Windows or Linux, but in
/// practice they always should be. The alternative would be to serialize them
/// as raw bytes, but on Windows that requires converting to/from UTF-16 which
/// is even more complicated.
///
/// Note: In the past (pre-1.8.0) this was encoded via MessagePack, which relied
/// on the `Serialize`/`Deserialize` implementation, which has the same
/// restrictions (it defers to the OS encoding).
#[derive(Clone, Debug, Display)]
#[display("{}", _0.display())]
pub struct CollectionPath(PathBuf);

impl CollectionPath {
    /// Get the canonical path for a collection file.
    ///
    /// This is fallible because it requires the path to exist. The path is
    /// canonicalized to deduplicate potential differences due to symlinks, cwd,
    /// etc. This ensures that any two references to the same file will always
    /// match the same
    pub fn try_from_path(path: &Path) -> Result<Self, DatabaseError> {
        path.canonicalize()
            .map_err(|error| DatabaseError::Path {
                path: path.to_owned(),
                error,
            })
            .traced()
            .map(Self)
    }

    /// Get the canonical path for a collection file, falling back to its
    /// normalized path if the file doesn't exist. The different between
    /// canonical and normalized is that canonical will resolve symlinks. This
    /// should only be used for collection lookups, and not inserts into the
    /// `collections` table. It provides a best guess at a match for collection
    /// files that may no longer be present.
    ///
    /// ## Errors
    ///
    /// Fails if the path is relative and the current working directory does not
    /// exist.
    pub fn try_from_path_maybe_missing(
        path: &Path,
    ) -> Result<Self, DatabaseError> {
        // Try to canonicalize first
        Self::try_from_path(path).or_else(|_| {
            let base = if path.is_relative() {
                env::current_dir().map_err(|error| DatabaseError::Path {
                    path: path.to_owned(),
                    error,
                })?
            } else {
                // Path is absolute - no need to append it to a base path
                PathBuf::new()
            };
            Ok(Self(paths::normalize_path(&base, path)))
        })
    }
}

impl From<CollectionPath> for PathBuf {
    fn from(value: CollectionPath) -> Self {
        value.0
    }
}

#[cfg(test)]
impl From<PathBuf> for CollectionPath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

/// Serialize path as UTF-8
impl ToSql for CollectionPath {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        #[derive(Debug, Error)]
        #[error("Collection path `{0:?}` is not valid UTF-8 as UTF-8")]
        struct PathStringifyError(PathBuf);

        self.0
            .to_str()
            .ok_or_else(|| {
                rusqlite::Error::ToSqlConversionFailure(
                    PathStringifyError(self.0.clone()).into(),
                )
            })?
            .as_bytes()
            .to_sql()
    }
}

/// Deserialize path from UTF-8
impl FromSql for CollectionPath {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        #[derive(Debug, Error)]
        #[error("Error parsing collection path as UTF-8")]
        struct PathParseError(Utf8Error);

        let path = str::from_utf8(value.as_blob()?)
            .map_err(PathParseError)
            .map_err(error_other)?
            .to_owned();
        Ok(Self(path.into()))
    }
}

/// Convert from `SELECT * FROM collections`
impl<'a, 'b> TryFrom<&'a Row<'b>> for CollectionMetadata {
    type Error = rusqlite::Error;

    fn try_from(row: &'a Row<'b>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get("id")?,
            path: row.get::<_, CollectionPath>("path")?.0,
            name: row.get("name")?,
        })
    }
}

/// A wrapper to serialize/deserialize a value as JSON for DB storage
#[derive(Debug)]
pub struct JsonEncoded<T>(pub T);

impl<T: Serialize> ToSql for JsonEncoded<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let s = serde_json::to_string(&self.0).map_err(|err| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(err))
        })?;
        Ok(ToSqlOutput::Owned(s.into()))
    }
}

impl<T: DeserializeOwned> FromSql for JsonEncoded<T> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        let value: T = serde_json::from_str(s).map_err(error_other)?;
        Ok(Self(value))
    }
}

/// Convert from `SELECT * FROM requests_v2`
impl<'a, 'b> TryFrom<&'a Row<'b>> for Exchange {
    type Error = rusqlite::Error;

    fn try_from(row: &'a Row<'b>) -> Result<Self, Self::Error> {
        let id: RequestId = row.get("id")?;
        Ok(Self {
            id,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            request: Arc::new(RequestRecord {
                id,
                profile_id: row.get("profile_id")?,
                recipe_id: row.get("recipe_id")?,
                http_version: row.get("http_version")?,
                method: row.get("method")?,
                // Use wrappers for all of these to specify the conversion
                url: row.get::<_, SqlWrap<_>>("url")?.0,
                headers: row.get::<_, SqlWrap<HeaderMap>>("request_headers")?.0,
                body: row
                    .get::<_, Option<SqlWrap<Bytes>>>("request_body")?
                    .map(|wrap| wrap.0),
            }),
            response: Arc::new(ResponseRecord {
                id,
                status: row.get::<_, SqlWrap<StatusCode>>("status_code")?.0,
                headers: row
                    .get::<_, SqlWrap<HeaderMap>>("response_headers")?
                    .0,
                body: row.get::<_, SqlWrap<Bytes>>("response_body")?.0.into(),
            }),
        })
    }
}

/// Convert from `SELECT ... FROM requests_v2`
impl<'a, 'b> TryFrom<&'a Row<'b>> for ExchangeSummary {
    type Error = rusqlite::Error;

    fn try_from(row: &'a Row<'b>) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.get("id")?,
            recipe_id: row.get("recipe_id")?,
            profile_id: row.get("profile_id")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            status: row.get::<_, SqlWrap<StatusCode>>("status_code")?.0,
        })
    }
}

/// A wrapper to define `ToSql`/`FromSql` impls on foreign types, to get around
/// the orphan rule
pub struct SqlWrap<T>(pub T);

impl FromSql for SqlWrap<Url> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str()?.parse().map(Self).map_err(error_other)
    }
}

impl FromSql for SqlWrap<Bytes> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        // Clone is necessary because the bytes live in sqlite FFI land
        let bytes = value.as_blob()?.to_owned();
        Ok(Self(bytes.into()))
    }
}

impl FromSql for SqlWrap<StatusCode> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let code: u16 = value.as_i64()?.try_into().map_err(error_other)?;
        let code = StatusCode::from_u16(code as u16).map_err(error_other)?;
        Ok(Self(code))
    }
}

// Serialize header map using the same format it gets in HTTP: key:value, one
// entry per line. The spec disallows colors in keys and newlines in values so
// it's safe to use both as delimiters

/// Char between header name and value
const HEADER_FIELD_DELIM: u8 = b':';
/// Char between header lines
/// <https://www.rfc-editor.org/rfc/rfc9110.html#name-field-values>
const HEADER_LINE_DELIM: u8 = b'\n';

impl ToSql for SqlWrap<&HeaderMap> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        // We know the exact capacity we'll need so we can avoid reallocations
        let capacity = self
            .0
            .iter()
            .map(|(name, value)| {
                // Include extra bytes for the delimiters
                name.as_str().len() + 1 + value.as_bytes().len() + 1
            })
            .sum();
        let mut buf: Vec<u8> = Vec::with_capacity(capacity);

        for (name, value) in self.0 {
            buf.extend(name.as_str().as_bytes());
            buf.push(HEADER_FIELD_DELIM);
            buf.extend(value.as_bytes());
            buf.push(HEADER_LINE_DELIM);
        }

        Ok(buf.into())
    }
}

impl FromSql for SqlWrap<HeaderMap> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        // There's no easy way to re-use the header parsing logic from the http
        // crate, so we have to reimplement it ourselves

        fn header_line(
            input: &mut &[u8],
        ) -> ModalResult<(HeaderName, HeaderValue)> {
            (
                terminated(
                    take_while(1.., |c| c != HEADER_FIELD_DELIM)
                        .try_map(HeaderName::from_bytes),
                    HEADER_FIELD_DELIM,
                ),
                terminated(
                    take_while(0.., |c| c != HEADER_LINE_DELIM)
                        .try_map(HeaderValue::from_bytes),
                    HEADER_LINE_DELIM,
                ),
            )
                .parse_next(input)
        }

        let bytes = value.as_blob()?;
        let lines = repeat(0.., header_line)
            .fold(HeaderMap::new, |mut acc, (name, value)| {
                acc.insert(name, value);
                acc
            })
            .parse(bytes)
            .map_err(|error| {
                /// This is the only way I could figure out to convert the parse
                /// error to something that implements `std:error:Error`
                /// https://github.com/winnow-rs/winnow/discussions/329
                #[derive(Debug, Error)]
                #[error("{0}")]
                struct HeaderParseError(String);

                error_other(HeaderParseError(error.to_string()))
            })?;
        Ok(Self(lines))
    }
}

impl ToSql for ProfileFilter<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Self::None => None::<&ProfileId>.to_sql(),
            Self::Some(id) => id.to_sql(),
            // This filter value shouldn't actually be used, but we can
            // serialize as null just to get a value
            Self::All => None::<&ProfileId>.to_sql(),
        }
    }
}

/// Create an `Other` variant of [FromSqlError]
fn error_other<T>(error: T) -> FromSqlError
where
    T: 'static + std::error::Error + Send + Sync,
{
    FromSqlError::Other(Box::new(error))
}
