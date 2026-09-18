//! The compiled self-contained forensic interface representation.

use std::io::{self, Cursor, ErrorKind, Read as _};
use std::pin::Pin;
use std::task::{Context, Poll};

use flate2::bufread::GzDecoder;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Frame, SizeHint};
use hyper::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HeaderValue, VARY, X_CONTENT_TYPE_OPTIONS,
};
use hyper::{Response, StatusCode};

use crate::body::{BodyError, WebBody};
use crate::encoding::{ContentCoding, etag_matches};

// build.rs validates this bundle and supplies hashes, lengths, and the script CSP.
const UI_GZIP: &[u8] = include_bytes!("../ui/kronika-ui.html.gz");
const UI_GZIP_ETAG: &str = env!("KRONIKA_UI_GZIP_ETAG");
const UI_IDENTITY_ETAG: &str = env!("KRONIKA_UI_IDENTITY_ETAG");
#[cfg(test)]
const UI_IDENTITY_SHA256: &str = env!("KRONIKA_UI_IDENTITY_SHA256");
const UI_CSP: &str = env!("KRONIKA_UI_CSP");
const UI_GZIP_LEN: &str = env!("KRONIKA_UI_GZIP_LEN");
const UI_IDENTITY_LEN: &str = env!("KRONIKA_UI_IDENTITY_LEN");
const UI_VARY: &str = "Authorization, Accept-Encoding";
// Bound each decode step and yield between chunks so a slow client cannot monopolize a worker.
const UI_IDENTITY_CHUNK_BYTES: usize = 8 * 1_024;

pub(crate) fn is_path(path: &str) -> bool {
    matches!(path, "/" | "/index.html")
}

pub(crate) fn response(
    head: bool,
    if_none_match: Option<&str>,
    coding: ContentCoding,
) -> io::Result<Response<WebBody>> {
    response_inner(
        head,
        if_none_match,
        coding,
        #[cfg(test)]
        tests::DecodeProbe::default(),
    )
}

fn response_inner(
    head: bool,
    if_none_match: Option<&str>,
    coding: ContentCoding,
    #[cfg(test)] probe: tests::DecodeProbe,
) -> io::Result<Response<WebBody>> {
    let (length, etag) = match coding {
        ContentCoding::Identity => (UI_IDENTITY_LEN, UI_IDENTITY_ETAG),
        ContentCoding::Gzip => (UI_GZIP_LEN, UI_GZIP_ETAG),
    };
    let not_modified = if_none_match.is_some_and(|offered| etag_matches(offered, etag));
    let body = match (head || not_modified, coding) {
        (true, _) => Full::new(Bytes::new())
            .map_err(BodyError::from)
            .boxed_unsync(),
        (false, ContentCoding::Gzip) => Full::new(Bytes::from_static(UI_GZIP))
            .map_err(BodyError::from)
            .boxed_unsync(),
        (false, ContentCoding::Identity) => {
            let expected_len = identity_len()?;
            #[cfg(not(test))]
            let body = IdentityBody::new(Bytes::from_static(UI_GZIP), expected_len);
            #[cfg(test)]
            let body = IdentityBody {
                probe,
                ..IdentityBody::new(Bytes::from_static(UI_GZIP), expected_len)
            };
            body.boxed_unsync()
        }
    };
    let mut response = Response::new(body);
    *response.status_mut() = if not_modified {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    if let Some(content_encoding) = coding.header() {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(content_encoding));
    }
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static(length));
    headers.insert(ETAG, HeaderValue::from_static(etag));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private,no-cache"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(UI_CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    set_vary(&mut response);
    Ok(response)
}

fn identity_len() -> io::Result<usize> {
    UI_IDENTITY_LEN.parse::<usize>().map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid embedded UI identity length: {error}"),
        )
    })
}

struct IdentityBody {
    decoder: Option<GzDecoder<Cursor<Bytes>>>,
    pending: Option<Bytes>,
    expected_len: usize,
    decoded: usize,
    emitted: usize,
    #[cfg(test)]
    started: bool,
    yield_before_decode: bool,
    #[cfg(test)]
    probe: tests::DecodeProbe,
}

impl IdentityBody {
    fn new(gzip: Bytes, expected_len: usize) -> Self {
        Self {
            decoder: Some(GzDecoder::new(Cursor::new(gzip))),
            pending: None,
            expected_len,
            decoded: 0,
            emitted: 0,
            #[cfg(test)]
            started: false,
            yield_before_decode: false,
            #[cfg(test)]
            probe: tests::DecodeProbe::default(),
        }
    }

    fn fail(
        &mut self,
        error: impl std::fmt::Display,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        eprintln!("kronika-web: stream embedded interface: {error}");
        self.decoder = None;
        self.pending = None;
        #[cfg(test)]
        self.probe.failed();
        Poll::Ready(Some(Err(BodyError)))
    }

    fn emit(&mut self, bytes: Bytes) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        self.emitted += bytes.len();
        self.yield_before_decode = true;
        #[cfg(test)]
        self.probe.emitted(bytes.len());
        Poll::Ready(Some(Ok(Frame::data(bytes))))
    }
}

impl hyper::body::Body for IdentityBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().get_mut();
        if this.decoder.is_none() && this.pending.is_none() {
            return Poll::Ready(None);
        }
        if this.yield_before_decode {
            this.yield_before_decode = false;
            #[cfg(test)]
            this.probe.yielded();
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        #[cfg(test)]
        if !this.started {
            this.started = true;
            this.probe.started();
        }
        loop {
            let Some(stream) = this.decoder.as_mut() else {
                return Poll::Ready(None);
            };
            let mut chunk = vec![0; UI_IDENTITY_CHUNK_BYTES];
            let read = match stream.read(&mut chunk) {
                Ok(read) => read,
                Err(error) => return this.fail(error),
            };
            if read == 0 {
                let Some(finished) = this.decoder.take() else {
                    return this.fail("embedded UI decoder stopped unexpectedly");
                };
                let cursor = finished.into_inner();
                if cursor.position() != cursor.get_ref().len() as u64 {
                    return this.fail("embedded UI gzip has trailing bytes");
                }
                if this.decoded != this.expected_len {
                    let actual = this.decoded;
                    let expected = this.expected_len;
                    return this.fail(format!(
                        "embedded UI identity length is {actual}, expected {expected}"
                    ));
                }
                #[cfg(test)]
                this.probe.completed();
                return this
                    .pending
                    .take()
                    .map_or(Poll::Ready(None), |pending| this.emit(pending));
            }
            chunk.truncate(read);
            let Some(next_len) = this.decoded.checked_add(read) else {
                return this.fail("embedded UI identity length overflowed");
            };
            if next_len > this.expected_len {
                let expected = this.expected_len;
                return this.fail(format!(
                    "embedded UI identity exceeds expected length {expected}"
                ));
            }
            this.decoded = next_len;
            let current = Bytes::from(chunk);
            if let Some(pending) = this.pending.replace(current) {
                return this.emit(pending);
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.decoder.is_none() && self.pending.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.expected_len.saturating_sub(self.emitted) as u64)
    }
}

pub(crate) fn set_vary(response: &mut Response<WebBody>) {
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(UI_VARY));
}

#[cfg(test)]
#[path = "tests/ui.rs"]
mod tests;
