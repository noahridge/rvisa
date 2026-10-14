//! Scripts describing how a mock instrument answers writes.

use std::sync::Arc;
use std::time::Duration;

/// What a mock instrument does when a rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Later responses become readable only after this delay.
    Delay(Duration),
    /// Queue a response message.
    Respond(Arc<[u8]>),
    /// Set the status byte (with RQS) and raise a service request.
    Srq(u8),
    /// Swallow the command: no response, so the next read times out.
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) pattern: Vec<u8>,
    pub(crate) actions: Vec<Action>,
}

/// How a mock instrument responds to writes.
///
/// Rules match a whole write exactly, including its terminator.
///
/// ```
/// use std::time::Duration;
/// use rvisa_mock::Script;
///
/// let script = Script::new()
///     .on_write("*IDN?\n").respond(b"ACME,Model 1,0001,1.0\n")
///     .on_write("MEAS?\n").delay(Duration::from_millis(50)).respond(b"1.25\n")
///     .on_write("HANG?\n").timeout()
///     .on_write("SRQ\n").srq(0x10).no_response();
/// ```
///
/// Writes that match no rule are recorded and ignored, as a real instrument
/// ignores commands it does not answer, unless the script is
/// [`strict`](Self::strict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    pub(crate) rules: Vec<Rule>,
    pub(crate) strict: bool,
    pub(crate) manufacturer: Option<String>,
    pub(crate) model: Option<String>,
}

impl Script {
    /// An empty script: every write is ignored.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a rule for writes equal to `command`.
    pub fn on_write(self, command: impl AsRef<[u8]>) -> RuleBuilder {
        RuleBuilder {
            script: self,
            rule: Rule {
                pattern: command.as_ref().to_vec(),
                actions: Vec::new(),
            },
        }
    }

    /// Makes unmatched writes fail with `Error::Protocol`.
    #[must_use]
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Sets the manufacturer and model reported by the
    /// `ManufacturerName` and `ModelName` attributes.
    #[must_use]
    pub fn identity(mut self, manufacturer: impl Into<String>, model: impl Into<String>) -> Self {
        self.manufacturer = Some(manufacturer.into());
        self.model = Some(model.into());
        self
    }

    pub(crate) fn find(&self, data: &[u8]) -> Option<&Rule> {
        self.rules.iter().find(|r| r.pattern == data)
    }
}

/// A rule being built by [`Script::on_write`]. Finish it with
/// [`respond`](Self::respond), [`no_response`](Self::no_response) or
/// [`timeout`](Self::timeout).
#[derive(Debug, Clone)]
#[must_use = "finish the rule with respond(), no_response() or timeout()"]
pub struct RuleBuilder {
    script: Script,
    rule: Rule,
}

impl RuleBuilder {
    /// Delays the response by `delay` after the write.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.rule.actions.push(Action::Delay(delay));
        self
    }

    /// Sets the status byte to `stb` with the RQS bit (0x40) and queues a
    /// service request event for sessions that enabled it.
    pub fn srq(mut self, stb: u8) -> Self {
        self.rule.actions.push(Action::Srq(stb));
        self
    }

    /// Answers with `response` as one message (END after its last byte).
    pub fn respond(mut self, response: impl AsRef<[u8]>) -> Script {
        self.rule
            .actions
            .push(Action::Respond(Arc::from(response.as_ref())));
        self.finish()
    }

    /// Accepts the command without answering.
    pub fn no_response(self) -> Script {
        self.finish()
    }

    /// Swallows the command, so the next read times out, as if the device
    /// hung. Unlike [`no_response`](Self::no_response), this is an explicit
    /// fault injection and is logged as such.
    pub fn timeout(mut self) -> Script {
        self.rule.actions.push(Action::Timeout);
        self.finish()
    }

    fn finish(mut self) -> Script {
        self.script.rules.push(self.rule);
        self.script
    }
}

/// Size of the block returned by the demo instruments' `BLOCK:LARGE?`.
pub const LARGE_BLOCK_LEN: usize = 1024 * 1024;

/// The script of the demonstration instruments: a generic SCPI device.
///
/// | Command | Response |
/// |---|---|
/// | `*IDN?\n` | `ACME Instruments,<model>,<serial>,1.0.0\n` |
/// | `*OPC?\n` | `1\n` |
/// | `*RST\n`, `*CLS\n`, `*CLS;*ESE 1;*SRE 32\n` | none |
/// | `*OPC\n` | none; raises a service request (STB 0x60, as after `*SRE 32`) |
/// | `MULTI?\n` | `line1\nline2\n` (one message) |
/// | `DATA?\n` | `#18` + two little-endian `f32`s (1.0, -2.5) + `\n` |
/// | `BLOCK:LARGE?\n` | a 1 MiB definite-length block + `\n` |
/// | `SRQ\n` | none; raises a service request (STB 0x50) |
/// | `SLOW?\n` | `done\n` after 200 ms |
/// | `HANG?\n` | never answers |
#[must_use]
pub fn demo_script(model: &str, serial: &str) -> Script {
    let mut data = b"#18".to_vec();
    data.extend_from_slice(&1.0f32.to_le_bytes());
    data.extend_from_slice(&(-2.5f32).to_le_bytes());
    data.push(b'\n');

    let mut large = rvisa_core::ieee488::encode_header(LARGE_BLOCK_LEN).into_bytes();
    // Every byte value appears, including the newline term char. Built by
    // bulk copies, which matters under Miri.
    let pattern: Vec<u8> = (0..=255u8).collect();
    large.extend_from_slice(&pattern.repeat(LARGE_BLOCK_LEN / pattern.len()));
    large.push(b'\n');

    Script::new()
        .identity("ACME Instruments", model)
        .on_write("*IDN?\n")
        .respond(format!("ACME Instruments,{model},{serial},1.0.0\n"))
        .on_write("*OPC?\n")
        .respond(b"1\n")
        .on_write("*RST\n")
        .no_response()
        .on_write("*CLS\n")
        .no_response()
        .on_write("*CLS;*ESE 1;*SRE 32\n")
        .no_response()
        .on_write("*OPC\n")
        .srq(0x20)
        .no_response()
        .on_write("MULTI?\n")
        .respond(b"line1\nline2\n")
        .on_write("DATA?\n")
        .respond(data)
        .on_write("BLOCK:LARGE?\n")
        .respond(large)
        .on_write("SRQ\n")
        .srq(0x10)
        .no_response()
        .on_write("SLOW?\n")
        .delay(Duration::from_millis(200))
        .respond(b"done\n")
        .on_write("HANG?\n")
        .timeout()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_collects_rules() {
        let script = Script::new()
            .on_write("A\n")
            .delay(Duration::from_millis(1))
            .srq(1)
            .respond(b"x")
            .on_write("B\n")
            .timeout()
            .strict();
        assert!(script.strict);
        assert_eq!(script.rules.len(), 2);
        assert_eq!(
            script.find(b"A\n").unwrap().actions,
            [
                Action::Delay(Duration::from_millis(1)),
                Action::Srq(1),
                Action::Respond(Arc::from(&b"x"[..]))
            ]
        );
        assert_eq!(script.find(b"B\n").unwrap().actions, [Action::Timeout]);
        assert!(script.find(b"C\n").is_none());
    }

    #[test]
    fn demo_large_block_is_well_formed() {
        let script = demo_script("M", "S");
        let Action::Respond(large) = &script.find(b"BLOCK:LARGE?\n").unwrap().actions[0] else {
            panic!("expected a response");
        };
        let data = rvisa_core::ieee488::parse_block(large).unwrap();
        assert_eq!(data.len(), LARGE_BLOCK_LEN);
        assert!(data.contains(&b'\n'));
    }
}
