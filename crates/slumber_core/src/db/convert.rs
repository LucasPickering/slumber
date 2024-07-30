//! Implementations to convert between Rust types and SQL data

use crate::{
    collection::{ProfileId, RecipeId},
    db::CollectionId,
    http::{
        Exchange, ExchangeSummary, RequestId, RequestRecord, ResponseRecord,
    },
    util::ResultTraced,
};
use anyhow::Context;
use bytes::Bytes;
use derive_more::Display;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, StatusCode,
};
use rusqlite::{
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    Row, ToSql,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::Debug,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use winnow::{
    combinator::{repeat, terminated},
    token::take_while,
    PResult, Parser,
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

/// Neat little wrapper for a collection path, to make sure it gets
/// canonicalized and serialized/deserialized consistently
#[derive(Debug, Display)]
#[display("{}", _0.0.display())]
pub struct CollectionPath(ByteEncoded<PathBuf>);

impl From<CollectionPath> for PathBuf {
    fn from(path: CollectionPath) -> Self {
        path.0 .0
    }
}

impl TryFrom<&Path> for CollectionPath {
    type Error = anyhow::Error;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        path.canonicalize()
            .context(format!("Error canonicalizing path {path:?}"))
            .traced()
            .map(|path| Self(ByteEncoded(path)))
    }
}

impl ToSql for CollectionPath {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for CollectionPath {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        ByteEncoded::<PathBuf>::column_result(value).map(Self)
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

/// A wrapper to serialize/deserialize a value as msgpack for DB storage
///
/// To be removed in https://github.com/LucasPickering/slumber/issues/306
#[derive(Debug)]
pub struct ByteEncoded<T>(pub T);

impl<T: Serialize> ToSql for ByteEncoded<T> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let bytes = rmp_serde::to_vec_named(&self.0).map_err(|err| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(err))
        })?;
        Ok(ToSqlOutput::Owned(bytes.into()))
    }
}

impl<T: DeserializeOwned> FromSql for ByteEncoded<T> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let bytes = value.as_blob()?;
        let value: T = rmp_serde::from_slice(bytes).map_err(error_other)?;
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
                // Use wrappers for all of these to specify the conversion
                method: row.get::<_, SqlWrap<_>>("method")?.0,
                url: row.get::<_, SqlWrap<_>>("url")?.0,
                headers: row.get::<_, SqlWrap<HeaderMap>>("request_headers")?.0,
                body: row
                    .get::<_, Option<SqlWrap<Bytes>>>("request_body")?
                    .map(|wrap| wrap.0),
            }),
            response: Arc::new(ResponseRecord {
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
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            status: row.get::<_, SqlWrap<StatusCode>>("status_code")?.0,
        })
    }
}

/// A wrapper to define `ToSql`/`FromSql` impls on foreign types, to get around
/// the orphan rule
pub struct SqlWrap<T>(pub T);

impl FromSql for SqlWrap<Method> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str()?.parse().map(Self).map_err(error_other)
    }
}

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
/// https://www.rfc-editor.org/rfc/rfc9110.html#name-field-values
const HEADER_LINE_DELIM: u8 = b'\n';

impl<'a> ToSql for SqlWrap<&'a HeaderMap> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        // We know the exact capacity we'll need so we can avoid reallocations
        let capacity = self
            .0
            .iter()
            .map(|(name, value)| {
                // Include extra bytes for the delimiters
                name.as_str().as_bytes().len() + 1 + value.as_bytes().len() + 1
            })
            .sum();
        let mut buf: Vec<u8> = Vec::with_capacity(capacity);

        for (name, value) in self.0.iter() {
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
        fn header_line(
            input: &mut &[u8],
        ) -> PResult<(HeaderName, HeaderValue)> {
            (
                terminated(
                    take_while(1.., |c| c != HEADER_FIELD_DELIM)
                        .try_map(HeaderName::from_bytes),
                    HEADER_FIELD_DELIM,
                ),
                terminated(
                    take_while(1.., |c| c != HEADER_LINE_DELIM)
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

/// Create an `Other` variant of [FromSqlError]
fn error_other<T>(error: T) -> FromSqlError
where
    T: 'static + std::error::Error + Send + Sync,
{
    FromSqlError::Other(Box::new(error))
}
