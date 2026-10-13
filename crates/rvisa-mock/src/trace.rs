//! The JSON Lines trace format (DESIGN.md M2).
//!
//! The first line is an [`Call::Open`] record; every later line is one
//! [`Session`](rvisa_core::Session) call with its arguments, result and
//! duration. Example:
//!
//! ```text
//! {"op":"open","resource":"TCPIP0::10.0.0.5::INSTR","backend":"vendor","timeout_us":2000000}
//! {"op":"write","data":"*IDN?\x0a","result":{"ok":6},"elapsed_us":412}
//! {"op":"read","max":65536,"result":{"ok":{"data":"ACME,1,2,3\x0a","reason":"end"}},"elapsed_us":2107}
//! ```

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use rvisa_core::{
    Attr, AttrValue, Error, EventKind, FlowControl, InterfaceType, LockKind, Parity, ReadResult,
    ResourceName, Result, StopBits, TermReason,
};
use serde::{Deserialize, Serialize};

pub use crate::bytes::Bytes;

/// One line of a trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The call.
    #[serde(flatten)]
    pub call: Call,
    /// How long the call took, in microseconds. Not replayed or compared.
    #[serde(default)]
    pub elapsed_us: u64,
}

/// A recorded call and its result.
#[allow(missing_docs)] // Fields mirror the `Session` method arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Call {
    /// The session was opened. Always the first entry.
    Open {
        resource: String,
        backend: String,
        /// The session's timeout when opened; `None` is infinite.
        timeout_us: Option<u64>,
    },
    Write {
        data: Bytes,
        result: Res<usize>,
    },
    Read {
        max: usize,
        result: Res<ReadRecord>,
    },
    SetTimeout {
        timeout_us: Option<u64>,
        result: Res<()>,
    },
    SetReadTermination {
        term: Option<u8>,
        result: Res<()>,
    },
    Clear {
        result: Res<()>,
    },
    ReadStb {
        result: Res<u8>,
    },
    AssertTrigger {
        result: Res<()>,
    },
    Lock {
        kind: LockRecord,
        timeout_us: u64,
        result: Res<()>,
    },
    Unlock {
        result: Res<()>,
    },
    GetAttr {
        attr: AttrRecord,
        result: Res<ValueRecord>,
    },
    SetAttr {
        attr: AttrRecord,
        value: ValueRecord,
        result: Res<()>,
    },
    EnableEvent {
        kind: EventRecord,
        result: Res<()>,
    },
    WaitEvent {
        kind: EventRecord,
        timeout_us: u64,
        result: Res<EventRecord>,
    },
    DisableEvent {
        kind: EventRecord,
        result: Res<()>,
    },
}

impl Call {
    /// The operation name, as it appears in the `op` field.
    #[must_use]
    pub fn op(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Write { .. } => "write",
            Self::Read { .. } => "read",
            Self::SetTimeout { .. } => "set_timeout",
            Self::SetReadTermination { .. } => "set_read_termination",
            Self::Clear { .. } => "clear",
            Self::ReadStb { .. } => "read_stb",
            Self::AssertTrigger { .. } => "assert_trigger",
            Self::Lock { .. } => "lock",
            Self::Unlock { .. } => "unlock",
            Self::GetAttr { .. } => "get_attr",
            Self::SetAttr { .. } => "set_attr",
            Self::EnableEvent { .. } => "enable_event",
            Self::WaitEvent { .. } => "wait_event",
            Self::DisableEvent { .. } => "disable_event",
        }
    }
}

/// A call's outcome: `{"ok": value}` or `{"err": {...}}`.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Res<T> {
    Ok(T),
    Err(ErrorRecord),
}

impl<T> Res<T> {
    /// Records a result.
    pub fn from_result<U>(result: &Result<U>, f: impl FnOnce(&U) -> T) -> Self {
        match result {
            Ok(v) => Self::Ok(f(v)),
            Err(e) => Self::Err(ErrorRecord::from(e)),
        }
    }

    /// Rebuilds the result.
    ///
    /// # Errors
    ///
    /// The recorded error, rebuilt with [`ErrorRecord::to_error`].
    pub fn into_result(self) -> Result<T> {
        match self {
            Self::Ok(v) => Ok(v),
            Self::Err(e) => Err(e.to_error()),
        }
    }
}

/// A recorded read.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRecord {
    pub data: Bytes,
    pub reason: ReasonRecord,
}

impl From<&ReadResult> for ReadRecord {
    fn from(r: &ReadResult) -> Self {
        Self {
            data: Bytes(r.data.clone()),
            reason: r.reason.into(),
        }
    }
}

impl ReadRecord {
    /// Rebuilds the read result.
    #[must_use]
    pub fn to_read_result(&self) -> ReadResult {
        ReadResult {
            data: self.data.0.clone(),
            reason: self.reason.into(),
        }
    }
}

/// A recorded [`TermReason`].
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonRecord {
    End,
    TermChar,
    MaxCount,
}

impl From<TermReason> for ReasonRecord {
    fn from(r: TermReason) -> Self {
        match r {
            TermReason::End => Self::End,
            TermReason::TermChar => Self::TermChar,
            // Reasons added later are recorded conservatively.
            _ => Self::MaxCount,
        }
    }
}

impl From<ReasonRecord> for TermReason {
    fn from(r: ReasonRecord) -> Self {
        match r {
            ReasonRecord::End => Self::End,
            ReasonRecord::TermChar => Self::TermChar,
            ReasonRecord::MaxCount => Self::MaxCount,
        }
    }
}

/// A recorded [`LockKind`].
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockRecord {
    Exclusive,
    Shared { key: String },
}

impl From<&LockKind> for LockRecord {
    fn from(k: &LockKind) -> Self {
        match k {
            LockKind::Exclusive => Self::Exclusive,
            LockKind::Shared { key } => Self::Shared { key: key.clone() },
        }
    }
}

/// A recorded [`EventKind`].
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventRecord {
    ServiceRequest,
    Trigger,
    Clear,
    Exception,
    UsbInterrupt,
    /// An event kind this trace format does not know.
    Unknown,
}

impl From<EventKind> for EventRecord {
    fn from(k: EventKind) -> Self {
        match k {
            EventKind::ServiceRequest => Self::ServiceRequest,
            EventKind::Trigger => Self::Trigger,
            EventKind::Clear => Self::Clear,
            EventKind::Exception => Self::Exception,
            EventKind::UsbInterrupt => Self::UsbInterrupt,
            _ => Self::Unknown,
        }
    }
}

impl EventRecord {
    /// The event kind, or `None` for [`EventRecord::Unknown`].
    #[must_use]
    pub fn to_kind(self) -> Option<EventKind> {
        Some(match self {
            Self::ServiceRequest => EventKind::ServiceRequest,
            Self::Trigger => EventKind::Trigger,
            Self::Clear => EventKind::Clear,
            Self::Exception => EventKind::Exception,
            Self::UsbInterrupt => EventKind::UsbInterrupt,
            Self::Unknown => return None,
        })
    }
}

/// A recorded [`Attr`].
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttrRecord {
    TimeoutValue,
    TermChar,
    TermCharEnabled,
    SendEndEnabled,
    SuppressEndEnabled,
    AsrlBaud,
    AsrlDataBits,
    AsrlParity,
    AsrlStopBits,
    AsrlFlowControl,
    TcpipKeepalive,
    TcpipNodelay,
    InterfaceType,
    ManufacturerName,
    ModelName,
    UsbSerialNumber,
    ResourceName,
    Raw(u32),
    /// An attribute this trace format does not know.
    Unknown,
}

impl From<Attr> for AttrRecord {
    fn from(a: Attr) -> Self {
        match a {
            Attr::TimeoutValue => Self::TimeoutValue,
            Attr::TermChar => Self::TermChar,
            Attr::TermCharEnabled => Self::TermCharEnabled,
            Attr::SendEndEnabled => Self::SendEndEnabled,
            Attr::SuppressEndEnabled => Self::SuppressEndEnabled,
            Attr::AsrlBaud => Self::AsrlBaud,
            Attr::AsrlDataBits => Self::AsrlDataBits,
            Attr::AsrlParity => Self::AsrlParity,
            Attr::AsrlStopBits => Self::AsrlStopBits,
            Attr::AsrlFlowControl => Self::AsrlFlowControl,
            Attr::TcpipKeepalive => Self::TcpipKeepalive,
            Attr::TcpipNodelay => Self::TcpipNodelay,
            Attr::InterfaceType => Self::InterfaceType,
            Attr::ManufacturerName => Self::ManufacturerName,
            Attr::ModelName => Self::ModelName,
            Attr::UsbSerialNumber => Self::UsbSerialNumber,
            Attr::ResourceName => Self::ResourceName,
            Attr::Raw(id) => Self::Raw(id),
            _ => Self::Unknown,
        }
    }
}

impl AttrRecord {
    /// The attribute, or `None` for [`AttrRecord::Unknown`].
    #[must_use]
    pub fn to_attr(self) -> Option<Attr> {
        Some(match self {
            Self::TimeoutValue => Attr::TimeoutValue,
            Self::TermChar => Attr::TermChar,
            Self::TermCharEnabled => Attr::TermCharEnabled,
            Self::SendEndEnabled => Attr::SendEndEnabled,
            Self::SuppressEndEnabled => Attr::SuppressEndEnabled,
            Self::AsrlBaud => Attr::AsrlBaud,
            Self::AsrlDataBits => Attr::AsrlDataBits,
            Self::AsrlParity => Attr::AsrlParity,
            Self::AsrlStopBits => Attr::AsrlStopBits,
            Self::AsrlFlowControl => Attr::AsrlFlowControl,
            Self::TcpipKeepalive => Attr::TcpipKeepalive,
            Self::TcpipNodelay => Attr::TcpipNodelay,
            Self::InterfaceType => Attr::InterfaceType,
            Self::ManufacturerName => Attr::ManufacturerName,
            Self::ModelName => Attr::ModelName,
            Self::UsbSerialNumber => Attr::UsbSerialNumber,
            Self::ResourceName => Attr::ResourceName,
            Self::Raw(id) => Attr::Raw(id),
            Self::Unknown => return None,
        })
    }
}

/// A recorded [`AttrValue`].
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueRecord {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I32(i32),
    Str(String),
    /// Microseconds; `None` is infinite.
    TimeoutUs(Option<u64>),
    Parity(String),
    StopBits(String),
    FlowControl(u16),
    InterfaceType(String),
    /// A value this trace format does not know.
    Unknown(String),
}

impl From<&AttrValue> for ValueRecord {
    fn from(v: &AttrValue) -> Self {
        match v {
            AttrValue::Bool(b) => Self::Bool(*b),
            AttrValue::U8(x) => Self::U8(*x),
            AttrValue::U16(x) => Self::U16(*x),
            AttrValue::U32(x) => Self::U32(*x),
            AttrValue::U64(x) => Self::U64(*x),
            AttrValue::I32(x) => Self::I32(*x),
            AttrValue::Str(s) => Self::Str(s.clone()),
            AttrValue::Timeout(t) => Self::TimeoutUs(t.map(micros)),
            AttrValue::Parity(p) => Self::Parity(format!("{p:?}")),
            AttrValue::StopBits(s) => Self::StopBits(format!("{s:?}")),
            AttrValue::FlowControl(f) => Self::FlowControl(f.bits()),
            AttrValue::InterfaceType(i) => Self::InterfaceType(format!("{i:?}")),
            other => Self::Unknown(format!("{other:?}")),
        }
    }
}

impl ValueRecord {
    /// The value, or `None` if it cannot be rebuilt.
    #[must_use]
    pub fn to_value(&self) -> Option<AttrValue> {
        Some(match self {
            Self::Bool(b) => AttrValue::Bool(*b),
            Self::U8(x) => AttrValue::U8(*x),
            Self::U16(x) => AttrValue::U16(*x),
            Self::U32(x) => AttrValue::U32(*x),
            Self::U64(x) => AttrValue::U64(*x),
            Self::I32(x) => AttrValue::I32(*x),
            Self::Str(s) => AttrValue::Str(s.clone()),
            Self::TimeoutUs(t) => AttrValue::Timeout(t.map(Duration::from_micros)),
            Self::Parity(p) => AttrValue::Parity(match p.as_str() {
                "None" => Parity::None,
                "Odd" => Parity::Odd,
                "Even" => Parity::Even,
                "Mark" => Parity::Mark,
                "Space" => Parity::Space,
                _ => return None,
            }),
            Self::StopBits(s) => AttrValue::StopBits(match s.as_str() {
                "One" => StopBits::One,
                "OnePointFive" => StopBits::OnePointFive,
                "Two" => StopBits::Two,
                _ => return None,
            }),
            Self::FlowControl(bits) => {
                AttrValue::FlowControl(FlowControl::from_bits_truncate(*bits))
            }
            Self::InterfaceType(i) => AttrValue::InterfaceType(match i.as_str() {
                "Gpib" => InterfaceType::Gpib,
                "Vxi" => InterfaceType::Vxi,
                "GpibVxi" => InterfaceType::GpibVxi,
                "Asrl" => InterfaceType::Asrl,
                "Pxi" => InterfaceType::Pxi,
                "Tcpip" => InterfaceType::Tcpip,
                "Usb" => InterfaceType::Usb,
                _ => return None,
            }),
            Self::Unknown(_) => return None,
        })
    }
}

/// A recorded [`Error`]. Static strings (backend and operation names)
/// cannot be restored exactly; see [`to_error`](Self::to_error).
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErrorRecord {
    Timeout,
    NotFound {
        resource: String,
    },
    Io {
        message: String,
    },
    Protocol {
        backend: String,
        detail: String,
    },
    Vendor {
        status: i32,
        name: Option<String>,
        desc: String,
    },
    Unsupported {
        backend: String,
        op: String,
    },
    InvalidResource {
        message: String,
    },
    BackendUnavailable {
        backend: String,
        reason: String,
    },
    ResourceLocked,
    AttrReadOnly {
        attr: AttrRecord,
    },
    EventNotEnabled {
        event: EventRecord,
    },
    InvalidArgument {
        message: String,
    },
    InvalidData {
        message: String,
    },
    NoBackend {
        resource: String,
    },
    Other {
        message: String,
    },
}

impl From<&Error> for ErrorRecord {
    fn from(e: &Error) -> Self {
        match e {
            Error::Timeout => Self::Timeout,
            Error::NotFound(rn) => Self::NotFound {
                resource: rn.to_string(),
            },
            Error::Io(io) => Self::Io {
                message: io.to_string(),
            },
            Error::Protocol { backend, detail } => Self::Protocol {
                backend: (*backend).into(),
                detail: detail.clone(),
            },
            Error::Vendor { status, name, desc } => Self::Vendor {
                status: *status,
                name: name.map(Into::into),
                desc: desc.clone(),
            },
            Error::Unsupported { backend, op } => Self::Unsupported {
                backend: (*backend).into(),
                op: (*op).into(),
            },
            Error::InvalidResource(m) => Self::InvalidResource { message: m.clone() },
            Error::BackendUnavailable { backend, reason } => Self::BackendUnavailable {
                backend: (*backend).into(),
                reason: reason.clone(),
            },
            Error::ResourceLocked => Self::ResourceLocked,
            Error::AttrReadOnly(a) => Self::AttrReadOnly { attr: (*a).into() },
            Error::EventNotEnabled(k) => Self::EventNotEnabled { event: (*k).into() },
            Error::InvalidArgument(m) => Self::InvalidArgument { message: m.clone() },
            Error::InvalidData(m) => Self::InvalidData { message: m.clone() },
            Error::NoBackend(rn) => Self::NoBackend {
                resource: rn.to_string(),
            },
            other => Self::Other {
                message: other.to_string(),
            },
        }
    }
}

/// The `Session` operation names, so recorded `Unsupported` errors can be
/// rebuilt with their `&'static str`.
const OPS: [&str; 18] = [
    "write",
    "read",
    "set_timeout",
    "set_read_termination",
    "set_write_termination",
    "clear",
    "read_stb",
    "assert_trigger",
    "lock",
    "unlock",
    "get_attr",
    "set_attr",
    "enable_event",
    "wait_event",
    "disable_event",
    "open",
    "list",
    "query",
];

/// The backend name used in rebuilt errors.
pub const REPLAY_BACKEND: &str = "replay";

impl ErrorRecord {
    /// Rebuilds the error. Backend names become [`REPLAY_BACKEND`], vendor
    /// status names become `None`, and an unknown `op` becomes `"unknown"`.
    #[must_use]
    pub fn to_error(&self) -> Error {
        let parse = |s: &str| s.parse::<ResourceName>();
        let protocol = |detail: String| Error::Protocol {
            backend: REPLAY_BACKEND,
            detail,
        };
        match self.clone() {
            Self::Timeout => Error::Timeout,
            Self::NotFound { resource } => match parse(&resource) {
                Ok(rn) => Error::NotFound(rn),
                Err(_) => protocol(format!("not found: {resource}")),
            },
            Self::Io { message } => Error::Io(std::io::Error::other(message)),
            Self::Protocol { detail, .. } => protocol(detail),
            Self::Vendor { status, desc, .. } => Error::Vendor {
                status,
                name: None,
                desc,
            },
            Self::Unsupported { op, .. } => Error::Unsupported {
                backend: REPLAY_BACKEND,
                op: OPS.into_iter().find(|o| *o == op).unwrap_or("unknown"),
            },
            Self::InvalidResource { message } => Error::InvalidResource(message),
            Self::BackendUnavailable { reason, .. } => Error::BackendUnavailable {
                backend: REPLAY_BACKEND,
                reason,
            },
            Self::ResourceLocked => Error::ResourceLocked,
            Self::AttrReadOnly { attr } => match attr.to_attr() {
                Some(a) => Error::AttrReadOnly(a),
                None => protocol("read-only attribute".into()),
            },
            Self::EventNotEnabled { event } => match event.to_kind() {
                Some(k) => Error::EventNotEnabled(k),
                None => protocol("event not enabled".into()),
            },
            Self::InvalidArgument { message } => Error::InvalidArgument(message),
            Self::InvalidData { message } => Error::InvalidData(message),
            Self::NoBackend { resource } => match parse(&resource) {
                Ok(rn) => Error::NoBackend(rn),
                Err(_) => protocol(format!("no backend: {resource}")),
            },
            Self::Other { message } => protocol(message),
        }
    }
}

/// A duration in whole microseconds, saturating.
pub(crate) fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// A whole trace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    /// The entries, starting with [`Call::Open`].
    pub entries: Vec<Entry>,
}

impl Trace {
    /// Parses JSON Lines. Blank lines are ignored.
    ///
    /// # Errors
    ///
    /// [`Error::Io`], or [`Error::InvalidData`] naming the bad line.
    pub fn read_jsonl(reader: impl BufRead) -> Result<Self> {
        let mut entries = Vec::new();
        for (i, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str(&line)
                .map_err(|e| Error::InvalidData(format!("trace line {}: {e}", i + 1)))?;
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// Writes JSON Lines.
    ///
    /// # Errors
    ///
    /// [`Error::Io`].
    pub fn write_jsonl(&self, mut writer: impl Write) -> Result<()> {
        for entry in &self.entries {
            write_entry(&mut writer, entry)?;
        }
        Ok(())
    }

    /// Loads a trace file.
    ///
    /// # Errors
    ///
    /// As [`read_jsonl`](Self::read_jsonl).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        Self::read_jsonl(std::io::BufReader::new(file))
    }

    /// Saves a trace file.
    ///
    /// # Errors
    ///
    /// [`Error::Io`].
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        self.write_jsonl(&mut file)?;
        file.flush()?;
        Ok(())
    }

    /// The instrument's response messages, in order: the data of successful
    /// reads, joined until a read ends with END. Stops at the term char do
    /// not split a message, since they depend on session settings rather
    /// than on what the device sent. This is what the golden trace
    /// comparison (ROADMAP §7.3) checks; timing is ignored. On transports
    /// without END (raw sockets) everything forms one stream.
    #[must_use]
    pub fn responses(&self) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        let mut current = Vec::new();
        for entry in &self.entries {
            if let Call::Read {
                result: Res::Ok(read),
                ..
            } = &entry.call
            {
                current.extend_from_slice(&read.data.0);
                if read.reason == ReasonRecord::End {
                    messages.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            messages.push(current);
        }
        messages
    }
}

pub(crate) fn write_entry(writer: &mut impl Write, entry: &Entry) -> Result<()> {
    serde_json::to_writer(&mut *writer, entry).map_err(|e| Error::Io(std::io::Error::other(e)))?;
    writer.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format() {
        let entry = Entry {
            call: Call::Write {
                data: Bytes(b"*IDN?\n".to_vec()),
                result: Res::Ok(6),
            },
            elapsed_us: 412,
        };
        let line = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            line,
            r#"{"op":"write","data":"*IDN?\\x0a","result":{"ok":6},"elapsed_us":412}"#
        );
        assert_eq!(serde_json::from_str::<Entry>(&line).unwrap(), entry);
    }

    #[test]
    fn every_error_round_trips() {
        let rn: ResourceName = "GPIB0::1".parse().unwrap();
        let errors = [
            Error::Timeout,
            Error::NotFound(rn.clone()),
            Error::Protocol {
                backend: "x",
                detail: "d".into(),
            },
            Error::Vendor {
                status: -5,
                name: Some("N"),
                desc: "d".into(),
            },
            Error::Unsupported {
                backend: "x",
                op: "read_stb",
            },
            Error::InvalidResource("m".into()),
            Error::BackendUnavailable {
                backend: "x",
                reason: "r".into(),
            },
            Error::ResourceLocked,
            Error::AttrReadOnly(Attr::ModelName),
            Error::EventNotEnabled(EventKind::ServiceRequest),
            Error::InvalidArgument("m".into()),
            Error::InvalidData("m".into()),
            Error::NoBackend(rn),
        ];
        for error in &errors {
            let record = ErrorRecord::from(error);
            let json = serde_json::to_string(&record).unwrap();
            let back: ErrorRecord = serde_json::from_str(&json).unwrap();
            assert_eq!(back, record);
            let rebuilt = back.to_error();
            assert_eq!(
                std::mem::discriminant(&rebuilt),
                std::mem::discriminant(error),
                "{error:?}"
            );
        }
        let io = ErrorRecord::from(&Error::Io(std::io::Error::other("boom")));
        assert!(matches!(io.to_error(), Error::Io(_)));
        let op = ErrorRecord::Unsupported {
            backend: "b".into(),
            op: "frobnicate".into(),
        };
        assert!(matches!(
            op.to_error(),
            Error::Unsupported { op: "unknown", .. }
        ));
        for bad in [
            ErrorRecord::NotFound {
                resource: "??".into(),
            },
            ErrorRecord::NoBackend {
                resource: "??".into(),
            },
            ErrorRecord::AttrReadOnly {
                attr: AttrRecord::Unknown,
            },
            ErrorRecord::EventNotEnabled {
                event: EventRecord::Unknown,
            },
            ErrorRecord::Other {
                message: "m".into(),
            },
        ] {
            assert!(matches!(bad.to_error(), Error::Protocol { .. }));
        }
    }

    #[test]
    fn every_value_round_trips() {
        let values = [
            AttrValue::Bool(true),
            AttrValue::U8(1),
            AttrValue::U16(2),
            AttrValue::U32(3),
            AttrValue::U64(4),
            AttrValue::I32(-5),
            AttrValue::Str("s".into()),
            AttrValue::Timeout(None),
            AttrValue::Timeout(Some(Duration::from_millis(1500))),
            AttrValue::Parity(Parity::None),
            AttrValue::Parity(Parity::Odd),
            AttrValue::Parity(Parity::Even),
            AttrValue::Parity(Parity::Mark),
            AttrValue::Parity(Parity::Space),
            AttrValue::StopBits(StopBits::One),
            AttrValue::StopBits(StopBits::OnePointFive),
            AttrValue::StopBits(StopBits::Two),
            AttrValue::FlowControl(FlowControl::RTS_CTS | FlowControl::XON_XOFF),
            AttrValue::InterfaceType(InterfaceType::Gpib),
            AttrValue::InterfaceType(InterfaceType::Vxi),
            AttrValue::InterfaceType(InterfaceType::GpibVxi),
            AttrValue::InterfaceType(InterfaceType::Asrl),
            AttrValue::InterfaceType(InterfaceType::Pxi),
            AttrValue::InterfaceType(InterfaceType::Tcpip),
            AttrValue::InterfaceType(InterfaceType::Usb),
        ];
        for value in values {
            assert_eq!(ValueRecord::from(&value).to_value(), Some(value));
        }
        for bad in [
            ValueRecord::Parity("x".into()),
            ValueRecord::StopBits("x".into()),
            ValueRecord::InterfaceType("x".into()),
            ValueRecord::Unknown("x".into()),
        ] {
            assert_eq!(bad.to_value(), None);
        }
    }

    #[test]
    fn every_attr_and_event_round_trips() {
        let attrs = [
            Attr::TimeoutValue,
            Attr::TermChar,
            Attr::TermCharEnabled,
            Attr::SendEndEnabled,
            Attr::SuppressEndEnabled,
            Attr::AsrlBaud,
            Attr::AsrlDataBits,
            Attr::AsrlParity,
            Attr::AsrlStopBits,
            Attr::AsrlFlowControl,
            Attr::TcpipKeepalive,
            Attr::TcpipNodelay,
            Attr::InterfaceType,
            Attr::ManufacturerName,
            Attr::ModelName,
            Attr::UsbSerialNumber,
            Attr::ResourceName,
            Attr::Raw(7),
        ];
        for attr in attrs {
            assert_eq!(AttrRecord::from(attr).to_attr(), Some(attr));
        }
        for kind in [
            EventKind::ServiceRequest,
            EventKind::Trigger,
            EventKind::Clear,
            EventKind::Exception,
            EventKind::UsbInterrupt,
        ] {
            assert_eq!(EventRecord::from(kind).to_kind(), Some(kind));
        }
        for reason in [TermReason::End, TermReason::TermChar, TermReason::MaxCount] {
            assert_eq!(TermReason::from(ReasonRecord::from(reason)), reason);
        }
    }

    #[test]
    fn bad_lines_are_reported() {
        let err = Trace::read_jsonl("\n{\"op\":\"nope\"}\n".as_bytes()).unwrap_err();
        assert!(err.to_string().contains("line 2"), "{err}");
    }
}
