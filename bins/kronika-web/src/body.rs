//! Bounded response staging, compression, and Hyper body delivery.

use std::convert::Infallible;
use std::io::{self, Write as _};
use std::pin::Pin;
use std::task::{Context, Poll};

use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use hyper::body::{Bytes, Frame, SizeHint};
use tokio::sync::{mpsc, oneshot};

use crate::api::{ApiError, ResponseMeta};
use crate::encoding::{AcceptedEncodings, ContentCoding};

const STAGED_PREFIX_BYTES: usize = 8 * 1_024;
pub(crate) const BODY_CHUNK_BYTES: usize = 8 * 1_024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyError;

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("response body failed")
    }
}

impl std::error::Error for BodyError {}

impl From<Infallible> for BodyError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

pub(crate) type BodyItem = Result<Vec<u8>, BodyError>;

pub(crate) struct StreamHead {
    pub(crate) meta: ResponseMeta,
    pub(crate) coding: Option<ContentCoding>,
}

impl StreamHead {
    pub(crate) const fn not_modified(meta: ResponseMeta) -> Self {
        Self { meta, coding: None }
    }
}

pub(crate) struct BodyProducer {
    accepted: AcceptedEncodings,
    meta: Option<ResponseMeta>,
    head: Option<oneshot::Sender<Result<StreamHead, ApiError>>>,
    body: mpsc::Sender<BodyItem>,
    state: ProducerState,
}

enum ProducerState {
    Staging(Vec<u8>),
    Identity,
    Gzip(GzEncoder<ChunkWriter>),
    Stopped,
}

impl BodyProducer {
    pub(crate) fn new(
        accepted: AcceptedEncodings,
        meta: ResponseMeta,
        head: oneshot::Sender<Result<StreamHead, ApiError>>,
        body: mpsc::Sender<BodyItem>,
    ) -> Self {
        Self {
            accepted,
            meta: Some(meta),
            head: Some(head),
            body,
            state: ProducerState::Staging(Vec::with_capacity(STAGED_PREFIX_BYTES)),
        }
    }

    pub(crate) fn emit(&mut self, bytes: &[u8]) -> bool {
        let state = std::mem::replace(&mut self.state, ProducerState::Stopped);
        match state {
            ProducerState::Staging(mut prefix) => {
                let remaining = STAGED_PREFIX_BYTES.saturating_sub(prefix.len());
                if bytes.len() <= remaining {
                    prefix.extend_from_slice(bytes);
                    self.state = ProducerState::Staging(prefix);
                    return true;
                }
                prefix.extend_from_slice(&bytes[..remaining]);
                if !self.start_stream(&prefix, self.accepted.for_large()) {
                    return false;
                }
                self.write_stream(&bytes[remaining..])
            }
            ProducerState::Identity => {
                self.state = ProducerState::Identity;
                self.write_stream(bytes)
            }
            ProducerState::Gzip(encoder) => {
                self.state = ProducerState::Gzip(encoder);
                self.write_stream(bytes)
            }
            ProducerState::Stopped => false,
        }
    }

    pub(crate) const fn can_restart(&self) -> bool {
        matches!(self.state, ProducerState::Staging(_))
    }

    pub(crate) fn restart(&mut self, meta: ResponseMeta) -> bool {
        let ProducerState::Staging(prefix) = &mut self.state else {
            return false;
        };
        prefix.clear();
        self.meta = Some(meta);
        true
    }

    pub(crate) fn complete_not_modified(mut self, meta: ResponseMeta) {
        self.state = ProducerState::Stopped;
        self.meta = None;
        if let Some(head) = self.head.take() {
            let _sent = head.send(Ok(StreamHead::not_modified(meta)));
        }
    }

    pub(crate) fn complete(mut self) {
        let state = std::mem::replace(&mut self.state, ProducerState::Stopped);
        match state {
            ProducerState::Staging(prefix) => {
                self.complete_prefix(&prefix, self.accepted.for_small());
            }
            ProducerState::Identity | ProducerState::Stopped => {}
            ProducerState::Gzip(encoder) => match finish_gzip(encoder) {
                Ok(()) => {}
                Err(_error) if self.body.is_closed() => {}
                Err(error) => {
                    let error = ApiError::Unreadable(Box::new(error));
                    self.fail_started(&error);
                }
            },
        }
    }

    /// Reject before headers; abort the body after the response begins.
    pub(crate) fn fail(mut self, error: ApiError) {
        let state = std::mem::replace(&mut self.state, ProducerState::Stopped);
        match state {
            ProducerState::Staging(_prefix) => self.reject(error),
            ProducerState::Gzip(mut encoder) => {
                encoder.get_mut().abort();
                drop(encoder);
                if !self.body.is_closed() {
                    self.fail_started(&error);
                }
            }
            ProducerState::Identity if !self.body.is_closed() => self.fail_started(&error),
            ProducerState::Identity | ProducerState::Stopped => {}
        }
    }

    fn start_stream(&mut self, prefix: &[u8], coding: ContentCoding) -> bool {
        let state = match coding {
            ContentCoding::Identity => {
                if send_identity(&self.body, prefix).is_err() {
                    return false;
                }
                ProducerState::Identity
            }
            ContentCoding::Gzip => {
                let mut encoder = gzip_encoder(self.body.clone());
                if let Err(error) = encoder.write_all(prefix) {
                    self.reject(ApiError::Unreadable(Box::new(error)));
                    return false;
                }
                ProducerState::Gzip(encoder)
            }
        };
        if !self.send_head(coding) {
            return false;
        }
        self.state = state;
        true
    }

    fn complete_prefix(&mut self, prefix: &[u8], coding: ContentCoding) {
        let result = match coding {
            ContentCoding::Identity => send_identity(&self.body, prefix),
            ContentCoding::Gzip => {
                let mut encoder = gzip_encoder(self.body.clone());
                encoder
                    .write_all(prefix)
                    .and_then(|()| finish_gzip(encoder))
            }
        };
        match result {
            Ok(()) => {
                let _sent = self.send_head(coding);
            }
            Err(error) => self.reject(ApiError::Unreadable(Box::new(error))),
        }
    }

    fn write_stream(&mut self, bytes: &[u8]) -> bool {
        let state = std::mem::replace(&mut self.state, ProducerState::Stopped);
        match state {
            ProducerState::Identity => {
                if send_identity(&self.body, bytes).is_ok() {
                    self.state = ProducerState::Identity;
                    true
                } else {
                    false
                }
            }
            ProducerState::Gzip(mut encoder) => {
                if encoder.write_all(bytes).is_ok() {
                    self.state = ProducerState::Gzip(encoder);
                    true
                } else {
                    false
                }
            }
            ProducerState::Staging(_) | ProducerState::Stopped => false,
        }
    }

    fn send_head(&mut self, coding: ContentCoding) -> bool {
        let Some(meta) = self.meta.take() else {
            return false;
        };
        let Some(head) = self.head.take() else {
            return false;
        };
        head.send(Ok(StreamHead {
            meta,
            coding: Some(coding),
        }))
        .is_ok()
    }

    fn reject(&mut self, error: ApiError) {
        self.meta = None;
        if let Some(head) = self.head.take() {
            let _sent = head.send(Err(error));
        }
    }

    fn fail_started(&self, error: &ApiError) {
        eprintln!("kronika-web: streamed resource failed: {error}");
        let _sent = self.body.blocking_send(Err(BodyError));
    }
}

fn gzip_encoder(body: mpsc::Sender<BodyItem>) -> GzEncoder<ChunkWriter> {
    GzBuilder::new()
        .mtime(0)
        .write(ChunkWriter::new(body), Compression::fast())
}

fn finish_gzip(encoder: GzEncoder<ChunkWriter>) -> io::Result<()> {
    encoder.finish()?.finish()
}

fn send_identity(body: &mpsc::Sender<BodyItem>, bytes: &[u8]) -> io::Result<()> {
    for chunk in bytes.chunks(BODY_CHUNK_BYTES) {
        body.blocking_send(Ok(chunk.to_vec()))
            .map_err(|_closed| disconnected())?;
    }
    Ok(())
}

pub(crate) struct ChunkWriter {
    body: mpsc::Sender<BodyItem>,
    bytes: Vec<u8>,
    aborted: bool,
}

impl ChunkWriter {
    pub(crate) fn new(body: mpsc::Sender<BodyItem>) -> Self {
        Self {
            body,
            bytes: Vec::with_capacity(BODY_CHUNK_BYTES),
            aborted: false,
        }
    }

    fn abort(&mut self) {
        self.bytes.clear();
        self.aborted = true;
    }

    fn send_pending(&mut self) -> io::Result<()> {
        if self.aborted {
            return Err(disconnected());
        }
        if self.bytes.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::replace(&mut self.bytes, Vec::with_capacity(BODY_CHUNK_BYTES));
        self.body
            .blocking_send(Ok(bytes))
            .map_err(|_closed| disconnected())
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.send_pending()
    }
}

impl io::Write for ChunkWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        if self.aborted {
            return Err(disconnected());
        }
        let written = buf.len();
        while !buf.is_empty() {
            let remaining = BODY_CHUNK_BYTES.saturating_sub(self.bytes.len());
            let take = remaining.min(buf.len());
            self.bytes.extend_from_slice(&buf[..take]);
            buf = &buf[take..];
            if self.bytes.len() == BODY_CHUNK_BYTES {
                self.send_pending()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_pending()
    }
}

fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "response body receiver closed")
}

pub(crate) struct ChannelBody {
    pub(crate) receiver: mpsc::Receiver<BodyItem>,
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.receiver
            .poll_recv(cx)
            .map(|item| item.map(|result| result.map(|bytes| Frame::data(Bytes::from(bytes)))))
    }

    fn is_end_stream(&self) -> bool {
        self.receiver.is_closed() && self.receiver.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
