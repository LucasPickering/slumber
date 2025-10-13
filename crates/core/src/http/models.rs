//! HTTP-related data types. The primary term here to know is "exchange". An
//! exchange is a single HTTP request-response pair. It can be in various
//! stages, meaning the request or response may not actually be present, if the
//! exchange is incomplete or failed.

use crate::{
    collection::{
        Authentication, ProfileId, RecipeBody, RecipeId, UnknownRecipeError,
    },
    http::content_type::ContentType,
};
use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use derive_more::{Display, From, FromStr};
use itertools::Itertools;
use mime::Mime;
use reqwest::{
    Body, Client, Request, StatusCode, Url,
    header::{self, HeaderMap, InvalidHeaderName, InvalidHeaderValue},
};
use serde::{Deserialize, Serialize};
use slumber_template::{RenderError, Template};
use std::{
    collections::HashMap, error::Error, fmt::Debug, io, str::Utf8Error,
    sync::Arc,
};
use strum::{EnumIter, IntoEnumIterator};
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

/// Unique ID for a single request. Can also be used to refer to the
/// corresponding [Exchange] or [ResponseRecord].
#[derive(
    Copy,
    Clone,
    Debug,
    Display,
    Eq,
    FromStr,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
)]
pub struct RequestId(pub Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP protocl version. This is duplicated from [reqwest::Version] because
/// that type doesn't provide any way to construct it. It only allows you to use
/// the existing constants.
#[derive(Copy, Clone, Debug, Default, EnumIter, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(into = "&str", try_from = "String")]
pub enum HttpVersion {
    Http09,
    Http10,
    #[default]
    Http11,
    Http2,
    Http3,
}

impl HttpVersion {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Http09 => "HTTP/0.9",
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
            Self::Http2 => "HTTP/2.0",
            Self::Http3 => "HTTP/3.0",
        }
    }
}

impl Display for HttpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.to_str())
    }
}

impl From<reqwest::Version> for HttpVersion {
    fn from(version: reqwest::Version) -> Self {
        match version {
            reqwest::Version::HTTP_09 => Self::Http09,
            reqwest::Version::HTTP_10 => Self::Http10,
            reqwest::Version::HTTP_11 => Self::Http11,
            reqwest::Version::HTTP_2 => Self::Http2,
            reqwest::Version::HTTP_3 => Self::Http3,
            _ => panic!("Unrecognized HTTP version: {version:?}"),
        }
    }
}

impl FromStr for HttpVersion {
    type Err = HttpVersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "HTTP/0.9" => Ok(Self::Http09),
            "HTTP/1.0" => Ok(Self::Http10),
            "HTTP/1.1" => Ok(Self::Http11),
            "HTTP/2.0" => Ok(Self::Http2),
            "HTTP/3.0" => Ok(Self::Http3),
            _ => Err(HttpVersionParseError {
                input: s.to_owned(),
            }),
        }
    }
}

/// For serialization
impl From<HttpVersion> for &'static str {
    fn from(version: HttpVersion) -> Self {
        version.to_str()
    }
}

/// For deserialization
impl TryFrom<String> for HttpVersion {
    type Error = <Self as FromStr>::Err;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[derive(Debug, Error)]
#[error(
    "Invalid HTTP version `{input}`. Must be one of: {}",
    HttpVersion::iter().map(HttpVersion::to_str).format(", "),
)]
pub struct HttpVersionParseError {
    input: String,
}

/// [HTTP request method](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Methods)
// This is duplicated from [reqwest::Method] so we can enforce
// the method is valid during deserialization. This is also generally more
// ergonomic at the cost of some flexibility.
#[derive(Copy, Clone, Debug, EnumIter, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
// Use FromStr to enable case-insensitivity
#[serde(into = "&str", try_from = "String")]
// Show as a string enum
#[cfg_attr(feature = "schema", schemars(!try_from, rename_all = "UPPERCASE"))]
pub enum HttpMethod {
    Connect,
    Delete,
    Get,
    Head,
    Options,
    Patch,
    Post,
    Put,
    Trace,
}

impl HttpMethod {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Connect => "CONNECT",
            Self::Delete => "DELETE",
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Patch => "PATCH",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Trace => "TRACE",
        }
    }
}

impl Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for HttpMethod {
    type Err = HttpMethodParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "CONNECT" => Ok(Self::Connect),
            "DELETE" => Ok(Self::Delete),
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            "OPTIONS" => Ok(Self::Options),
            "PATCH" => Ok(Self::Patch),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "TRACE" => Ok(Self::Trace),
            _ => Err(HttpMethodParseError {
                input: s.to_owned(),
            }),
        }
    }
}

impl From<&reqwest::Method> for HttpMethod {
    fn from(method: &reqwest::Method) -> Self {
        // reqwest supports custom methods, but we don't provide any
        // mechanism for users to use them, so we should never panic
        method.as_str().parse().unwrap()
    }
}

/// For serialization
impl From<HttpMethod> for &'static str {
    fn from(method: HttpMethod) -> Self {
        method.to_str()
    }
}

/// For deserialization
impl TryFrom<String> for HttpMethod {
    type Error = <Self as FromStr>::Err;

    fn try_from(method: String) -> Result<Self, Self::Error> {
        method.parse()
    }
}

#[derive(Debug, Error)]
#[error(
    "Invalid HTTP method `{input}`. Must be one of: {}",
    HttpMethod::iter().map(HttpMethod::to_str).format(", "),
)]
pub struct HttpMethodParseError {
    input: String,
}

/// The first stage in building a request. This contains the initialization data
/// needed to build a request. This holds owned data because we need to be able
/// to move it between tasks as part of the build process, which requires it
/// to be `'static`.
pub struct RequestSeed {
    /// Unique ID for this request
    pub id: RequestId,
    /// Recipe from which the request should be rendered
    pub recipe_id: RecipeId,
    /// Configuration for the build
    pub options: BuildOptions,
}

impl RequestSeed {
    pub fn new(recipe_id: RecipeId, options: BuildOptions) -> Self {
        Self {
            id: RequestId::new(),
            recipe_id,
            options,
        }
    }
}

/// Options for modifying a recipe during a build, corresponding to changes the
/// user can make in the TUI (as opposed to the collection file). This is
/// helpful for applying temporary modifications made by the user. By providing
/// this in a separate struct, we prevent the need to clone, modify, and pass
/// recipes everywhere. Recipes could be very large so cloning may be expensive,
/// and this options layer makes the available modifications clear and
/// restricted.
///
/// These store *indexes* rather than keys because keys may not be necessarily
/// unique (e.g. in the case of query params). Technically some could use keys
/// and some could use indexes, but I chose consistency.
#[derive(Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct BuildOptions {
    /// URL can be overridden but not disabled
    pub url: Option<Template>,
    /// Authentication can be overridden, but not disabled. For simplicity,
    /// the override is wholesale rather than by field.
    pub authentication: Option<Authentication>,
    pub headers: BuildFieldOverrides,
    pub query_parameters: BuildFieldOverrides,
    pub form_fields: BuildFieldOverrides,
    /// Override body. This should *not* be used for form bodies, since those
    /// can be overridden on a field-by-field basis.
    pub body: Option<RecipeBody>,
}

/// A collection of modifications made to a particular section of a recipe
/// (query params, headers, etc.). See [BuildFieldOverride]
#[derive(Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct BuildFieldOverrides {
    overrides: HashMap<usize, BuildFieldOverride>,
}

impl BuildFieldOverrides {
    /// Get the value to be used for a particular field, keyed by index. Return
    /// `None` if the field should be dropped from the request, and use the
    /// given default if no override is provided.
    pub fn get<'a>(
        &'a self,
        index: usize,
        default: &'a Template,
    ) -> Option<&'a Template> {
        match self.overrides.get(&index) {
            Some(BuildFieldOverride::Omit) => None,
            Some(BuildFieldOverride::Override(template)) => Some(template),
            None => Some(default),
        }
    }
}

impl FromIterator<(usize, BuildFieldOverride)> for BuildFieldOverrides {
    fn from_iter<T: IntoIterator<Item = (usize, BuildFieldOverride)>>(
        iter: T,
    ) -> Self {
        Self {
            overrides: HashMap::from_iter(iter),
        }
    }
}

/// Modifications made to a single field (query param, header, etc.) in a
/// recipe
#[derive(Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub enum BuildFieldOverride {
    /// Do not include this field in the recipe
    Omit,
    /// Replace the value for this field with a different template
    Override(Template),
}

/// A request ready to be launched into through the stratosphere. This is
/// basically a two-part ticket: the request is the part we'll hand to the HTTP
/// engine to be launched, and the record is the ticket stub we'll keep for
/// ourselves (to display to the user
#[derive(Debug)]
pub struct RequestTicket {
    /// A record of the request that we can hang onto and persist
    pub(super) record: Arc<RequestRecord>,
    /// reqwest client that should be used to launch the request
    pub(super) client: Client,
    /// Our brave little astronaut, ready to be launched...
    pub(super) request: Request,
}

impl RequestTicket {
    pub fn record(&self) -> &Arc<RequestRecord> {
        &self.record
    }
}

/// A complete request+response pairing. This is generated by
/// [RequestTicket::send] when a response is received successfully for a sent
/// request. This is cheaply cloneable because the request and response are
/// both wrapped in `Arc`.
#[derive(Clone, Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct Exchange {
    /// ID to uniquely refer to this exchange
    pub id: RequestId,
    /// What we said. Use an Arc so the view can hang onto it.
    pub request: Arc<RequestRecord>,
    /// What we heard
    pub response: Arc<ResponseRecord>,
    /// When was the request sent to the server?
    pub start_time: DateTime<Utc>,
    /// When did we finish receiving the *entire* response?
    pub end_time: DateTime<Utc>,
}

impl Exchange {
    /// Get the elapsed time for this request
    pub fn duration(&self) -> Duration {
        self.end_time - self.start_time
    }

    pub fn summary(&self) -> ExchangeSummary {
        ExchangeSummary {
            id: self.id,
            recipe_id: self.request.recipe_id.clone(),
            profile_id: self.request.profile_id.clone(),
            start_time: self.start_time,
            end_time: self.end_time,
            status: self.response.status,
        }
    }
}

/// Metadata about an exchange. Useful in lists where request/response content
/// isn't needed.
#[derive(Clone, Debug, PartialEq)]
pub struct ExchangeSummary {
    pub id: RequestId,
    pub recipe_id: RecipeId,
    pub profile_id: Option<ProfileId>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub status: StatusCode,
}

/// Data for an HTTP request. This is similar to [reqwest::Request], but differs
/// in some key ways:
/// - Each [reqwest::Request] can only exist once (from creation to sending),
///   whereas a record can be hung onto after the launch to keep showing it on
///   screen.
/// - This stores additional Slumber-specific metadata
///
/// This intentionally does *not* implement `Clone`, because request data could
/// potentially be large so we want to be intentional about duplicating it only
/// when necessary.
#[derive(Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct RequestRecord {
    /// Unique ID for this request
    pub id: RequestId,
    /// The profile used to render this request (for historical context)
    pub profile_id: Option<ProfileId>,
    /// The recipe used to generate this request (for historical context)
    pub recipe_id: RecipeId,

    /// HTTP protocol version. Unlike `method`, we can't use the reqwest type
    /// here because there's way to externally construct the type.
    pub http_version: HttpVersion,
    /// HTTP method
    pub method: HttpMethod,
    /// URL, including query params/fragment
    pub url: Url,
    pub headers: HeaderMap,
    /// Body content as bytes. This should be decoded as needed. This will
    /// **not** be populated for bodies that are above the "large" threshold.
    /// - `Some(empty bytes)`: There was no body (e.g. GET request)
    /// - `None`: Body couldn't be stored (stream or too large)
    pub body: Option<Bytes>,
}

impl RequestRecord {
    /// Create a new request record from data and metadata. This is the
    /// canonical way to create a record for a new request. This should
    /// *not* be build directly, and instead the data should copy data out of
    /// a [reqwest::Request]. This is to prevent duplicating request
    /// construction logic.
    ///
    /// This will clone all data out of the request. This could potentially be
    /// expensive but we don't have any choice if we want to send it to the
    /// server and show it in the TUI at the same time
    pub(super) fn new(
        seed: RequestSeed,
        profile_id: Option<ProfileId>,
        request: &Request,
        max_body_size: usize,
    ) -> Self {
        Self {
            id: seed.id,
            profile_id,
            recipe_id: seed.recipe_id,

            http_version: request.version().into(),
            method: request.method().into(),
            url: request.url().clone(),
            headers: request.headers().clone(),
            body: request
                .body()
                // Stream bodies and bodies over a certain size threshold are
                // thrown away. Storing request bodies in general doesn't
                // provide a ton of value, so we shouldn't do it at the expense
                // of performance
                .and_then(Body::as_bytes)
                .filter(|body| body.len() <= max_body_size)
                .map(|body| body.to_owned().into()),
        }
    }

    /// Get the value of the request's `Content-Type` header, if any
    pub fn mime(&self) -> Option<Mime> {
        content_type_header(&self.headers)
    }

    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for RequestRecord {
    fn factory((): ()) -> Self {
        Self::factory((RequestId::new(), None, RecipeId::factory(())))
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<RequestId> for RequestRecord {
    fn factory(id: RequestId) -> Self {
        Self::factory((id, None, RecipeId::factory(())))
    }
}

/// Customize profile and recipe ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<(Option<ProfileId>, RecipeId)> for RequestRecord {
    fn factory((profile_id, recipe_id): (Option<ProfileId>, RecipeId)) -> Self {
        Self::factory((RequestId::new(), profile_id, recipe_id))
    }
}

/// Customize request, profile and recipe ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<(RequestId, Option<ProfileId>, RecipeId)>
    for RequestRecord
{
    fn factory(
        (id, profile_id, recipe_id): (RequestId, Option<ProfileId>, RecipeId),
    ) -> Self {
        use crate::test_util::header_map;
        Self {
            id,
            profile_id,
            recipe_id,
            method: HttpMethod::Get,
            http_version: HttpVersion::Http11,
            url: "http://localhost/url".parse().unwrap(),
            headers: header_map([
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
                ("User-Agent", "slumber"),
            ]),
            body: None,
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for ResponseRecord {
    fn factory((): ()) -> Self {
        Self::factory(RequestId::new())
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<RequestId> for ResponseRecord {
    fn factory(id: RequestId) -> Self {
        Self {
            id,
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: ResponseBody::default(),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<StatusCode> for ResponseRecord {
    fn factory(status: StatusCode) -> Self {
        Self {
            id: RequestId::new(),
            status,
            headers: HeaderMap::new(),
            body: ResponseBody::default(),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Exchange {
    fn factory((): ()) -> Self {
        Self::factory((None, RecipeId::factory(())))
    }
}

/// Customize recipe ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<RecipeId> for Exchange {
    fn factory(params: RecipeId) -> Self {
        Self::factory((None, params))
    }
}

/// Customize request, profile, and recipe ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<(RequestId, Option<ProfileId>, RecipeId)>
    for Exchange
{
    fn factory(
        (id, profile_id, recipe_id): (RequestId, Option<ProfileId>, RecipeId),
    ) -> Self {
        Self::factory((
            RequestRecord {
                id,
                ..RequestRecord::factory((profile_id, recipe_id))
            },
            ResponseRecord::factory(id),
        ))
    }
}

/// Customize profile and recipe ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<(Option<ProfileId>, RecipeId)> for Exchange {
    fn factory(params: (Option<ProfileId>, RecipeId)) -> Self {
        let id = RequestId::new();
        Self::factory((
            RequestRecord {
                id,
                ..RequestRecord::factory(params)
            },
            ResponseRecord::factory(id),
        ))
    }
}

/// Custom request and response
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<(RequestRecord, ResponseRecord)> for Exchange {
    fn factory((request, response): (RequestRecord, ResponseRecord)) -> Self {
        // Request and response should've been generated from the same ID,
        // otherwise we're going to see some shitty bugs
        assert_eq!(
            request.id, response.id,
            "Request and response have different IDs"
        );
        Self {
            id: request.id,
            request: request.into(),
            response: response.into(),
            start_time: Utc::now(),
            end_time: Utc::now(),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<RequestId> for Exchange {
    fn factory(id: RequestId) -> Self {
        Self::factory((RequestRecord::factory(id), ResponseRecord::factory(id)))
    }
}

/// A resolved HTTP response, with all content loaded and ready to be displayed
/// to the user. A simpler alternative to [reqwest::Response], because there's
/// no way to access all resolved data on that type at once. Resolving the
/// response body requires moving the response.
///
/// This intentionally does not implement Clone, because responses could
/// potentially be very large.
#[derive(Debug)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
pub struct ResponseRecord {
    pub id: RequestId,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: ResponseBody,
}

impl ResponseRecord {
    /// Get the value of the response's `Content-Type` header, if any
    pub fn mime(&self) -> Option<Mime> {
        content_type_header(&self.headers)
    }

    /// Get the value of the response's `Content-Type` header, and parse it as
    /// a known/supported content type
    pub fn content_type(&self) -> Option<ContentType> {
        ContentType::from_headers(&self.headers).ok()
    }

    /// Get a suggested file name for the content of this response. First we'll
    /// check the Content-Disposition header. If it's missing or doesn't have a
    /// file name, we'll check the Content-Type to at least guess at an
    /// extension.
    pub fn file_name(&self) -> Option<String> {
        self.headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| {
                // Parse header for the `filename="{}"` parameter
                // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Disposition
                let value = value.to_str().ok()?;
                value.split(';').find_map(|part| {
                    let (key, value) = part.trim().split_once('=')?;
                    if key == "filename" {
                        Some(value.trim_matches('"').to_owned())
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                // Grab the extension from the Content-Type header. Don't use
                // self.conten_type() because we want to accept unknown types.
                let content_type = self.headers.get(header::CONTENT_TYPE)?;
                let mime: Mime = content_type.to_str().ok()?.parse().ok()?;
                Some(format!("data.{}", mime.subtype()))
            })
    }
}

/// Get the value of the `Content-Type` header, parsed as a MIME. `None` if the
/// header isn't present or isn't a valid MIME type
fn content_type_header(headers: &HeaderMap) -> Option<Mime> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok()?.parse().ok())
}

/// HTTP response body. Content is stored as bytes because it may not
/// necessarily be valid UTF-8. Converted to text only as needed.
///
/// The generic type is to make this usable with references to bodies. In most
/// cases you can just use the default.
#[derive(Clone, Default)]
pub struct ResponseBody<T = Bytes> {
    /// Raw body
    data: T,
}

impl<T: AsRef<[u8]>> ResponseBody<T> {
    pub fn new(data: T) -> Self {
        Self { data }
    }

    /// Raw content bytes
    pub fn bytes(&self) -> &T {
        &self.data
    }

    /// Owned raw content bytes
    pub fn into_bytes(self) -> T {
        self.data
    }

    /// Get bytes as text, if valid UTF-8
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(self.data.as_ref()).ok()
    }

    /// Get body size, in bytes
    pub fn size(&self) -> usize {
        self.data.as_ref().len()
    }
}

impl Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't print the actual body because it could be huge
        f.debug_tuple("Body")
            .field(&format!("<{} bytes>", self.data.len()))
            .finish()
    }
}

impl<T: From<Bytes>> From<Bytes> for ResponseBody<T> {
    fn from(data: Bytes) -> Self {
        Self { data: data.into() }
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for ResponseBody {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned().into())
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&[u8]> for ResponseBody {
    fn from(value: &[u8]) -> Self {
        Self::new(value.to_owned().into())
    }
}

#[cfg(any(test, feature = "test"))]
impl From<serde_json::Value> for ResponseBody {
    fn from(value: serde_json::Value) -> Self {
        Self::new(value.to_string().into())
    }
}

#[cfg(any(test, feature = "test"))]
impl PartialEq for ResponseBody {
    fn eq(&self, other: &Self) -> bool {
        // Ignore derived data
        self.data == other.data
    }
}

/// An error that can occur while *building* a request
#[derive(Debug, Error)]
#[error("Error building request {id}")]
pub struct RequestBuildError {
    /// Underlying error
    #[source]
    pub error: RequestBuildErrorKind,

    /// ID of the profile being rendered under
    pub profile_id: Option<ProfileId>,
    /// ID of the recipe being rendered
    pub recipe_id: RecipeId,
    /// ID of the failed request
    pub id: RequestId,
    /// When did the build start?
    pub start_time: DateTime<Utc>,
    /// When did the build end, i.e. when did the error occur?
    pub end_time: DateTime<Utc>,
}

impl RequestBuildError {
    /// Does this error have *any* error in its chain that contains
    /// [TriggeredRequestError::NotAllowed]? This makes it easy to attach
    /// additional error context.
    pub fn has_trigger_disabled_error(&self) -> bool {
        // Walk down the error chain
        // unstable: Use error.sources()
        // https://github.com/rust-lang/rust/issues/58520
        let mut next: Option<&dyn Error> = Some(self);
        while let Some(error) = next {
            if matches!(
                error.downcast_ref(),
                Some(TriggeredRequestError::NotAllowed)
            ) {
                return true;
            }
            next = error.source();
        }
        false
    }
}

#[cfg(any(test, feature = "test"))]
impl PartialEq for RequestBuildError {
    fn eq(&self, other: &Self) -> bool {
        self.profile_id == other.profile_id
            && self.recipe_id == other.recipe_id
            && self.id == other.id
            && self.start_time == other.start_time
            && self.end_time == other.end_time
            && self.error.to_string() == other.error.to_string()
    }
}

/// The various errors that can occur while building a request. This provides
/// the error for [RequestBuildError], which then attaches additional context.
#[derive(Debug, Error)]
pub enum RequestBuildErrorKind {
    /// Error rendering username in Basic auth
    #[error("Rendering password")]
    AuthPasswordRender(#[source] RenderError),
    /// Error rendering token in Bearer auth
    #[error("Rendering bearer token")]
    AuthTokenRender(#[source] RenderError),
    /// Error rendering username in Basic auth
    #[error("Rendering username")]
    AuthUsernameRender(#[source] RenderError),

    /// Error streaming directly from a file to a request body (via reqwest)
    #[error("Streaming request body")]
    BodyFileStream(#[source] io::Error),
    /// Error rendering a body to bytes/stream
    #[error("Rendering form field `{field}`")]
    BodyFormFieldRender {
        field: String,
        #[source]
        error: RenderError,
    },
    /// Error rendering a body to bytes/stream
    #[error("Rendering body")]
    BodyRender(#[source] RenderError),
    /// Error while streaming bytes for a body
    #[error("Streaming request body")]
    BodyStream(#[source] RenderError),

    /// Error assembling the final request
    #[error(transparent)]
    Build(#[from] reqwest::Error),

    /// Header name does not meet the HTTP spec
    #[error("Invalid header name `{header}`")]
    HeaderInvalidName {
        header: String,
        #[source]
        error: InvalidHeaderName,
    },
    /// Header name does not meet the HTTP spec
    #[error("Invalid header name `{header}`")]
    HeaderInvalidValue {
        header: String,
        #[source]
        error: InvalidHeaderValue,
    },
    /// Header value does not meet the HTTP spec
    #[error("Invalid value for header `{header}`")]
    HeaderRender {
        header: String,
        #[source]
        error: RenderError,
    },

    /// Attempted to generate a cURL command for a request with non-UTF-8
    /// values, which we don't support representing in the generated command
    #[error("Non-text value in curl output")]
    CurlInvalidUtf8(#[source] Utf8Error),

    /// Error rendering query parameter
    #[error("Rendering query parameter `{parameter}`")]
    QueryRender {
        parameter: String,
        #[source]
        error: RenderError,
    },

    /// Tried to build a recipe that doesn't exist
    #[error(transparent)]
    RecipeUnknown(#[from] UnknownRecipeError),

    /// URL rendered correctly but the result isn't a valid URL
    #[error("Invalid URL")]
    UrlInvalid {
        url: String,
        #[source]
        error: url::ParseError,
    },
    /// Error rendering URL
    #[error("Rendering URL")]
    UrlRender(#[source] RenderError),
}

/// An error that can occur during a request. This does *not* including building
/// errors.
#[derive(Debug, Error)]
#[error(
    "Error executing request for `{}` (request `{}`)",
    .request.recipe_id,
    .request.id,
)]
pub struct RequestError {
    /// Underlying error
    #[source]
    pub error: reqwest::Error,

    /// The request that caused all this ruckus
    pub request: Arc<RequestRecord>,
    /// When was the request launched?
    pub start_time: DateTime<Utc>,
    /// When did the error occur?
    pub end_time: DateTime<Utc>,
}

#[cfg(any(test, feature = "test"))]
impl PartialEq for RequestError {
    fn eq(&self, other: &Self) -> bool {
        self.error.to_string() == other.error.to_string()
            && self.request == other.request
            && self.start_time == other.start_time
            && self.end_time == other.end_time
    }
}

/// Error fetching a previous request while rendering a new request
#[derive(Debug, Error)]
#[error(transparent)]
pub struct StoredRequestError(pub Box<dyn 'static + Error + Send + Sync>);

impl StoredRequestError {
    pub fn new<E: 'static + Error + Send + Sync>(error: E) -> Self {
        Self(Box::new(error))
    }
}

/// Error occurred while trying to build/execute a triggered request.
///
/// This type implements `Clone` so it can be shared between deduplicated chain
/// renders, hence the `Arc`s on inner errors.
#[derive(Clone, Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TriggeredRequestError {
    /// This render was invoked in a way that doesn't support automatic request
    /// execution. In some cases the user needs to explicitly opt in to enable
    /// it (e.g. with a CLI flag)
    #[error("Triggered request execution not allowed in this context")]
    NotAllowed,

    /// Tried to auto-execute a chained request but couldn't build it
    #[error(transparent)]
    Build(#[from] Arc<RequestBuildError>),

    /// Chained request was triggered, sent and failed
    #[error(transparent)]
    Send(#[from] Arc<RequestError>),
}

impl From<RequestBuildError> for TriggeredRequestError {
    fn from(error: RequestBuildError) -> Self {
        Self::Build(error.into())
    }
}

impl From<RequestError> for TriggeredRequestError {
    fn from(error: RequestError) -> Self {
        Self::Send(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::header_map;
    use indexmap::indexmap;
    use rstest::rstest;
    use slumber_util::Factory;

    #[rstest]
    #[case::content_disposition(
        ResponseRecord {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data;name=\"field\"; filename=\"fish.png\"",
                "content-type" => "image/png",
            }),
            ..ResponseRecord::factory(())
        },
        Some("fish.png")
    )]
    #[case::content_type_known(
        ResponseRecord {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data",
                "content-type" => "application/json",
            }),
            ..ResponseRecord::factory(())
        },
        Some("data.json")
    )]
    #[case::content_type_unknown(
        ResponseRecord {
            headers: header_map(indexmap! {
                "content-disposition" => "form-data",
                "content-type" => "image/jpeg",
            }),
            ..ResponseRecord::factory(())
        },
        Some("data.jpeg")
    )]
    #[case::none(ResponseRecord::factory(()), None)]
    fn test_file_name(
        #[case] response: ResponseRecord,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(response.file_name().as_deref(), expected);
    }
}
