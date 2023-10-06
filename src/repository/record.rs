//! Data types stored in the request repository

use crate::http::{Request, RequestId, Response};
use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use derive_more::Display;
use strum::{EnumDiscriminants, EnumString};

/// A single request+response in history
#[derive(Debug)]
pub struct RequestRecord {
    /// When was the request registered in history? This should be very close
    /// to when it was sent to the server
    pub start_time: DateTime<Utc>,
    pub request: Request,
    /// Current status of this request
    pub state: RequestState,
}

/// State of an HTTP response, which can be pending or completed. Also generate
/// a discriminant-only enum that will map to the `response_kind` column in the
/// database.
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(RequestStateKind), derive(Display, EnumString))]
pub enum RequestState {
    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading,
    /// The request never terminated because the program exited while it was
    /// in flight. We have no idea of knowing how long it took, so this is
    /// stored separately from the error state.
    Incomplete,

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response {
        response: Response,
        /// When did we finish receiving the full response?
        end_time: DateTime<Utc>,
    },
    /// Error occurred sending the request or receiving the response. We're
    /// never going to do anything with the error but display it, so just
    /// store it as a string. This makes it easy to display to the user and
    /// serialize/deserialize.
    Error {
        error: String,
        /// When did the error occur?
        end_time: DateTime<Utc>,
    },
}

impl RequestRecord {
    /// Get the unique ID for this request
    pub fn id(&self) -> RequestId {
        self.request.id
    }

    /// Unpack the request state as a response. If it isn't a
    /// success, return an error.
    pub fn try_response(&self) -> anyhow::Result<&Response> {
        match &self.state {
            RequestState::Response { response, .. } => Ok(response),
            other => Err(anyhow!("Request is in non-success state {other:?}")),
        }
    }

    /// Get the elapsed time for this request, according to request state:
    /// - Loading - Elapsed time since the request started
    /// - Incomplete - `None`
    /// - Response - Duration from start to loading the entire request
    /// - Error - Duration from start to request failing
    pub fn duration(&self) -> Option<Duration> {
        match &self.state {
            RequestState::Loading => Some(Utc::now() - self.start_time),
            RequestState::Incomplete => None,
            RequestState::Response { end_time, .. }
            | RequestState::Error { end_time, .. } => {
                Some(*end_time - self.start_time)
            }
        }
    }
}
