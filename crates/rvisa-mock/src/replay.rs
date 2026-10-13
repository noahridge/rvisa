//! `ReplaySession`: serves a recorded trace back.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use rvisa_core::{
    Attr, AttrValue, Error, Event, EventKind, LockKind, ReadResult, ResourceName, Result, Session,
};

use crate::bytes::escape;
use crate::trace::{Call, Entry, LockRecord, REPLAY_BACKEND, Trace, ValueRecord, micros};

/// A session that plays back a [`Trace`].
///
/// Each call must match the next recorded call: the same operation with
/// the same arguments. It then returns the recorded result. A mismatch is
/// [`Error::Protocol`] describing both calls. Timing is not replayed unless
/// [`realtime`](Self::realtime) is set.
///
/// ```
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_mock::{MockBackend, RecordingSession, ReplaySession, Trace};
///
/// // Record a query against the mock...
/// let backend = MockBackend::demo();
/// let session = backend.open(&"ASRL1::INSTR".parse()?, &OpenOptions::new())?;
/// let mut rec = RecordingSession::new(session, "mock", Vec::new())?;
/// let idn = rec.query_bytes(b"*IDN?\n")?;
/// let (_, jsonl) = rec.finish()?;
///
/// // ...and play it back.
/// let mut replay = ReplaySession::new(Trace::read_jsonl(jsonl.as_slice())?)?;
/// assert_eq!(replay.query_bytes(b"*IDN?\n")?, idn);
/// assert!(replay.is_finished());
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug)]
pub struct ReplaySession {
    resource: ResourceName,
    timeout: Option<Duration>,
    calls: RefCell<VecDeque<Entry>>,
    realtime: bool,
}

fn mismatch(expected: &Call, detail: &str) -> Error {
    Error::Protocol {
        backend: REPLAY_BACKEND,
        detail: format!(
            "expected recorded {} call ({expected:?}), but got {detail}",
            expected.op()
        ),
    }
}

impl ReplaySession {
    /// Prepares to replay `trace`, which must start with an `open` record.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidData`] if the trace does not start with a valid
    /// `open` record.
    pub fn new(trace: Trace) -> Result<Self> {
        let mut calls: VecDeque<Entry> = trace.entries.into_iter().collect();
        let Some(Entry {
            call:
                Call::Open {
                    resource,
                    timeout_us,
                    ..
                },
            ..
        }) = calls.pop_front()
        else {
            return Err(Error::InvalidData(
                "trace does not start with an open record".into(),
            ));
        };
        let resource = resource
            .parse()
            .map_err(|e| Error::InvalidData(format!("trace open record: {e}")))?;
        Ok(Self {
            resource,
            timeout: timeout_us.map(Duration::from_micros),
            calls: RefCell::new(calls),
            realtime: false,
        })
    }

    /// Makes each call take as long as it did when recorded, for code that
    /// measures time (such as timeout tests).
    #[must_use]
    pub fn realtime(mut self) -> Self {
        self.realtime = true;
        self
    }

    /// Whether every recorded call has been replayed.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.calls.borrow().is_empty()
    }

    /// The number of recorded calls not yet replayed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.calls.borrow().len()
    }

    /// Replays the next call if `check` accepts it: `check` returns `None`
    /// when the call's operation or arguments differ, which leaves the call
    /// in place and returns a mismatch error.
    fn next<T>(&self, describe: &str, check: impl FnOnce(&Call) -> Option<Result<T>>) -> Result<T> {
        let mut calls = self.calls.borrow_mut();
        let Some(entry) = calls.front() else {
            return Err(Error::Protocol {
                backend: REPLAY_BACKEND,
                detail: format!("trace exhausted, but got {describe}"),
            });
        };
        let Some(result) = check(&entry.call) else {
            return Err(mismatch(&entry.call, describe));
        };
        let elapsed = Duration::from_micros(entry.elapsed_us);
        calls.pop_front();
        drop(calls);
        if self.realtime {
            std::thread::sleep(elapsed);
        }
        result
    }
}

impl Session for ReplaySession {
    fn resource(&self) -> &ResourceName {
        &self.resource
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.next(&format!("write {}", escape(data)), |call| match call {
            Call::Write { data: d, result } if d.0 == data => Some(result.clone().into_result()),
            _ => None,
        })
    }

    fn read(&mut self, max: usize) -> Result<ReadResult> {
        self.next(&format!("read of up to {max} bytes"), |call| match call {
            Call::Read { max: m, result } if *m == max => {
                Some(result.clone().into_result().map(|r| r.to_read_result()))
            }
            _ => None,
        })
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        let result = self.next(&format!("set_timeout({timeout:?})"), |call| match call {
            Call::SetTimeout { timeout_us, result } if *timeout_us == timeout.map(micros) => {
                Some(result.clone().into_result())
            }
            _ => None,
        });
        if result.is_ok() {
            self.timeout = timeout;
        }
        result
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        self.next(
            &format!("set_read_termination({term:?})"),
            |call| match call {
                Call::SetReadTermination { term: t, result } if *t == term => {
                    Some(result.clone().into_result())
                }
                _ => None,
            },
        )
    }

    fn clear(&mut self) -> Result<()> {
        self.next("clear", |call| match call {
            Call::Clear { result } => Some(result.clone().into_result()),
            _ => None,
        })
    }

    fn read_stb(&mut self) -> Result<u8> {
        self.next("read_stb", |call| match call {
            Call::ReadStb { result } => Some(result.clone().into_result()),
            _ => None,
        })
    }

    fn assert_trigger(&mut self) -> Result<()> {
        self.next("assert_trigger", |call| match call {
            Call::AssertTrigger { result } => Some(result.clone().into_result()),
            _ => None,
        })
    }

    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let record = LockRecord::from(&kind);
        self.next(&format!("lock({kind:?}, {timeout:?})"), |call| match call {
            Call::Lock {
                kind: k,
                timeout_us,
                result,
            } if *k == record && *timeout_us == micros(timeout) => {
                Some(result.clone().into_result())
            }
            _ => None,
        })
    }

    fn unlock(&mut self) -> Result<()> {
        self.next("unlock", |call| match call {
            Call::Unlock { result } => Some(result.clone().into_result()),
            _ => None,
        })
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        self.next(&format!("get_attr({attr:?})"), |call| match call {
            Call::GetAttr { attr: a, result } if *a == attr.into() => {
                Some(result.clone().into_result().and_then(|value| {
                    value.to_value().ok_or_else(|| Error::Protocol {
                        backend: REPLAY_BACKEND,
                        detail: format!("cannot rebuild recorded value {value:?}"),
                    })
                }))
            }
            _ => None,
        })
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        let record = ValueRecord::from(&value);
        let result = self.next(
            &format!("set_attr({attr:?}, {value:?})"),
            |call| match call {
                Call::SetAttr {
                    attr: a,
                    value: v,
                    result,
                } if *a == attr.into() && *v == record => Some(result.clone().into_result()),
                _ => None,
            },
        );
        if result.is_ok()
            && let (Attr::TimeoutValue, AttrValue::Timeout(timeout)) = (attr, &value)
        {
            self.timeout = *timeout;
        }
        result
    }

    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        self.next(&format!("enable_event({kind:?})"), |call| match call {
            Call::EnableEvent { kind: k, result } if *k == kind.into() => {
                Some(result.clone().into_result())
            }
            _ => None,
        })
    }

    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        self.next(
            &format!("wait_event({kind:?}, {timeout:?})"),
            |call| match call {
                Call::WaitEvent {
                    kind: k,
                    timeout_us,
                    result,
                } if *k == kind.into() && *timeout_us == micros(timeout) => {
                    Some(result.clone().into_result().and_then(|event| {
                        event
                            .to_kind()
                            .map(Event::new)
                            .ok_or_else(|| Error::Protocol {
                                backend: REPLAY_BACKEND,
                                detail: "cannot rebuild recorded event".into(),
                            })
                    }))
                }
                _ => None,
            },
        )
    }

    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        self.next(&format!("disable_event({kind:?})"), |call| match call {
            Call::DisableEvent { kind: k, result } if *k == kind.into() => {
                Some(result.clone().into_result())
            }
            _ => None,
        })
    }
}
