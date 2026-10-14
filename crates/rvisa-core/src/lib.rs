//! Core types for `rvisa`: resource names, the [`Session`] and [`Backend`]
//! traits every transport implements, errors, attributes, events and
//! IEEE 488.2 helpers.
//!
//! Applications normally use the `rvisa` facade crate, which re-exports
//! these. Backend authors implement [`Backend`] and [`Session`].
//!
//! ```
//! use rvisa_core::ResourceName;
//!
//! let rn: ResourceName = "USB::0x0957::0x1796::MY123".parse()?;
//! assert_eq!(rn.to_string(), "USB0::0x0957::0x1796::MY123::INSTR");
//! # Ok::<(), rvisa_core::Error>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]

mod attr;
mod error;
mod event;
pub mod ieee488;
mod locks;
mod pattern;
mod resource;
mod session;

pub use attr::{Attr, AttrKind, AttrValue, FlowControl, InterfaceType, Parity, StopBits};
pub use error::{Error, Result};
pub use event::{Event, EventKind};
pub use locks::LockTable;
pub use pattern::Pattern;
pub use resource::ResourceName;
pub use session::{
    Backend, LockKind, OpenOptions, READ_CHUNK, ReadResult, Session, SessionExt, TermReason,
};
