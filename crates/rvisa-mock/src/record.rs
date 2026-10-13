//! `RecordingSession`: logs every call of a wrapped session to a trace.

use std::io::Write;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rvisa_core::{
    Attr, AttrValue, Error, Event, EventKind, LockKind, ReadResult, ResourceName, Result, Session,
};

use crate::bytes::Bytes;
use crate::trace::{Call, Entry, ReadRecord, Res, ValueRecord, micros, write_entry};

struct Writer<W> {
    out: W,
    /// The first error writing the trace; recording stops after it.
    failure: Option<Error>,
}

/// Wraps a session and writes each call, its arguments, result and
/// duration to a JSON Lines trace (see [`trace`](crate::trace)).
///
/// The wrapped session behaves exactly as before. If writing the trace
/// fails, recording stops and [`finish`](Self::finish) reports the error;
/// the session itself keeps working.
///
/// ```
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_mock::{MockBackend, RecordingSession, Trace};
///
/// let backend = MockBackend::demo();
/// let session = backend.open(&"GPIB0::22::INSTR".parse()?, &OpenOptions::new())?;
/// let mut recording = RecordingSession::new(session, "mock", Vec::new())?;
/// recording.query_bytes(b"*IDN?\n")?;
/// let (_session, jsonl) = recording.finish()?;
///
/// let trace = Trace::read_jsonl(jsonl.as_slice())?;
/// assert_eq!(trace.responses(), [b"ACME Instruments,PSU30,MOCK001,1.0.0\n"]);
/// # Ok::<(), rvisa_core::Error>(())
/// ```
pub struct RecordingSession<S: Session, W: Write + Send> {
    inner: S,
    writer: Mutex<Writer<W>>,
}

impl<S: Session, W: Write + Send> std::fmt::Debug for RecordingSession<S, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingSession")
            .field("resource", self.inner.resource())
            .finish_non_exhaustive()
    }
}

impl<S: Session, W: Write + Send> RecordingSession<S, W> {
    /// Starts recording `inner`, opened by `backend`, to `out`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the `open` record cannot be written.
    pub fn new(inner: S, backend: &str, mut out: W) -> Result<Self> {
        let open = Entry {
            call: Call::Open {
                resource: inner.resource().to_string(),
                backend: backend.to_owned(),
                timeout_us: inner.timeout().map(micros),
            },
            elapsed_us: 0,
        };
        write_entry(&mut out, &open)?;
        Ok(Self {
            inner,
            writer: Mutex::new(Writer { out, failure: None }),
        })
    }

    /// Stops recording and returns the wrapped session and the writer.
    ///
    /// # Errors
    ///
    /// The first error that occurred writing the trace, or flushing it.
    pub fn finish(self) -> Result<(S, W)> {
        let mut writer = self
            .writer
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(failure) = writer.failure.take() {
            return Err(failure);
        }
        writer.out.flush()?;
        Ok((self.inner, writer.out))
    }

    fn log(&self, start: Instant, call: Call) {
        let entry = Entry {
            call,
            elapsed_us: micros(start.elapsed()),
        };
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if writer.failure.is_none()
            && let Err(e) = write_entry(&mut writer.out, &entry)
        {
            tracing::warn!(error = %e, "recording stopped: could not write the trace");
            writer.failure = Some(e);
        }
    }
}

fn unit(_: &()) {}

impl<S: Session, W: Write + Send> Session for RecordingSession<S, W> {
    fn resource(&self) -> &ResourceName {
        self.inner.resource()
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let start = Instant::now();
        let result = self.inner.write(data);
        self.log(
            start,
            Call::Write {
                data: Bytes(data.to_vec()),
                result: Res::from_result(&result, |n| *n),
            },
        );
        result
    }

    fn read(&mut self, max: usize) -> Result<ReadResult> {
        let start = Instant::now();
        let result = self.inner.read(max);
        self.log(
            start,
            Call::Read {
                max,
                result: Res::from_result(&result, |r| ReadRecord::from(r)),
            },
        );
        result
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.set_timeout(timeout);
        self.log(
            start,
            Call::SetTimeout {
                timeout_us: timeout.map(micros),
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn timeout(&self) -> Option<Duration> {
        self.inner.timeout()
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.set_read_termination(term);
        self.log(
            start,
            Call::SetReadTermination {
                term,
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn write_termination(&self) -> Option<&[u8]> {
        self.inner.write_termination()
    }

    fn set_write_termination(&mut self, term: Option<&[u8]>) -> Result<()> {
        self.inner.set_write_termination(term)
    }

    fn clear(&mut self) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.clear();
        self.log(
            start,
            Call::Clear {
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn read_stb(&mut self) -> Result<u8> {
        let start = Instant::now();
        let result = self.inner.read_stb();
        self.log(
            start,
            Call::ReadStb {
                result: Res::from_result(&result, |s| *s),
            },
        );
        result
    }

    fn assert_trigger(&mut self) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.assert_trigger();
        self.log(
            start,
            Call::AssertTrigger {
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let record = (&kind).into();
        let result = self.inner.lock(kind, timeout);
        self.log(
            start,
            Call::Lock {
                kind: record,
                timeout_us: micros(timeout),
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn unlock(&mut self) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.unlock();
        self.log(
            start,
            Call::Unlock {
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        let start = Instant::now();
        let result = self.inner.get_attr(attr);
        self.log(
            start,
            Call::GetAttr {
                attr: attr.into(),
                result: Res::from_result(&result, |v| ValueRecord::from(v)),
            },
        );
        result
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        let start = Instant::now();
        let record = ValueRecord::from(&value);
        let result = self.inner.set_attr(attr, value);
        self.log(
            start,
            Call::SetAttr {
                attr: attr.into(),
                value: record,
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.enable_event(kind);
        self.log(
            start,
            Call::EnableEvent {
                kind: kind.into(),
                result: Res::from_result(&result, unit),
            },
        );
        result
    }

    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        let start = Instant::now();
        let result = self.inner.wait_event(kind, timeout);
        self.log(
            start,
            Call::WaitEvent {
                kind: kind.into(),
                timeout_us: micros(timeout),
                result: Res::from_result(&result, |e| e.kind.into()),
            },
        );
        result
    }

    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        let start = Instant::now();
        let result = self.inner.disable_event(kind);
        self.log(
            start,
            Call::DisableEvent {
                kind: kind.into(),
                result: Res::from_result(&result, unit),
            },
        );
        result
    }
}
