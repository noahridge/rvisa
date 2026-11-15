//! In-process VISA lock bookkeeping for backends whose transport has no
//! locking of its own (DESIGN.md C13).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::error::{Error, Result};
use crate::resource::ResourceName;
use crate::session::LockKind;

/// The locks held on one resource, by session ID.
///
/// It follows VISA's rules: an exclusive lock admits only its owner; a
/// shared lock admits every session that locked with the same key; locks
/// nest, and each [`try_lock`](Self::try_lock) needs an
/// [`unlock`](Self::unlock). A backend keeps one table per resource,
/// behind a mutex shared by that resource's sessions, and gives each
/// session a unique ID.
///
/// ```
/// use rvisa_core::{LockKind, LockTable};
///
/// let mut locks = LockTable::new();
/// assert!(locks.try_lock(1, &LockKind::Exclusive));
/// assert!(locks.blocks(2), "session 2 is locked out");
/// assert!(!locks.try_lock(2, &LockKind::Exclusive));
/// assert!(locks.unlock(1));
/// assert!(!locks.blocks(2));
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LockTable {
    /// The session holding an exclusive lock, and its nesting depth.
    exclusive: Option<(u64, u32)>,
    /// The shared lock's key and each holder's nesting depth.
    shared: Option<(String, HashMap<u64, u32>)>,
}

impl LockTable {
    /// No locks held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `session` is locked out of the resource by another
    /// session's lock.
    #[must_use]
    pub fn blocks(&self, session: u64) -> bool {
        let exclusive = self.exclusive.is_some_and(|(owner, _)| owner != session);
        let shared = self
            .shared
            .as_ref()
            .is_some_and(|(_, holders)| !holders.contains_key(&session));
        exclusive || shared
    }

    /// Takes a lock for `session`, or one more level of a lock it holds.
    /// Returns `false`, changing nothing, if another session's lock is in
    /// the way.
    pub fn try_lock(&mut self, session: u64, kind: &LockKind) -> bool {
        match kind {
            LockKind::Exclusive => match &mut self.exclusive {
                Some((owner, depth)) if *owner == session => {
                    *depth += 1;
                    true
                }
                Some(_) => false,
                None => {
                    let others_share = self
                        .shared
                        .as_ref()
                        .is_some_and(|(_, h)| h.keys().any(|&s| s != session));
                    if others_share {
                        return false;
                    }
                    self.exclusive = Some((session, 1));
                    true
                }
            },
            LockKind::Shared { key } => {
                if self.exclusive.is_some_and(|(owner, _)| owner != session) {
                    return false;
                }
                match &mut self.shared {
                    Some((held, holders)) if held == key => {
                        *holders.entry(session).or_insert(0) += 1;
                        true
                    }
                    Some(_) => false,
                    None => {
                        self.shared = Some((key.clone(), HashMap::from([(session, 1)])));
                        true
                    }
                }
            }
        }
    }

    /// Releases one level of `session`'s lock: its exclusive lock first,
    /// then its share of a shared lock. Returns `false` if it holds none.
    pub fn unlock(&mut self, session: u64) -> bool {
        if let Some((owner, depth)) = &mut self.exclusive
            && *owner == session
        {
            *depth -= 1;
            if *depth == 0 {
                self.exclusive = None;
            }
            return true;
        }
        if let Some((_, holders)) = &mut self.shared
            && let Some(depth) = holders.get_mut(&session)
        {
            *depth -= 1;
            if *depth == 0 {
                holders.remove(&session);
            }
            if holders.is_empty() {
                self.shared = None;
            }
            return true;
        }
        false
    }

    /// Releases every lock `session` holds, as when it closes.
    pub fn release_all(&mut self, session: u64) {
        while self.unlock(session) {}
    }
}

/// One [`LockTable`] per resource, for the sessions of one backend
/// (DESIGN.md C13). Each session takes an ID from
/// [`new_id`](Self::new_id) and passes it to the other methods.
///
/// ```
/// use rvisa_core::{Error, LockKind, LockRegistry, ResourceName};
///
/// let rn: ResourceName = "TCPIP0::h::5025::SOCKET".parse()?;
/// let locks = LockRegistry::new();
/// let (a, b) = (locks.new_id(), locks.new_id());
/// assert!(locks.try_lock(&rn, a, &LockKind::Exclusive));
/// assert!(matches!(locks.check(&rn, b), Err(Error::ResourceLocked)));
/// locks.release_all(&rn, a);
/// locks.check(&rn, b)?;
/// # Ok::<(), Error>(())
/// ```
#[derive(Debug, Default)]
pub struct LockRegistry {
    tables: Mutex<HashMap<ResourceName, LockTable>>,
    next_id: AtomicU64,
}

impl LockRegistry {
    /// No locks held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A new session ID.
    pub fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn with<R>(&self, resource: &ResourceName, f: impl FnOnce(&mut LockTable) -> R) -> R {
        let mut tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        let table = tables.entry(resource.clone()).or_default();
        let result = f(table);
        if *table == LockTable::new() {
            tables.remove(resource);
        }
        result
    }

    /// Fails if another session's lock keeps `session` out of `resource`.
    ///
    /// # Errors
    ///
    /// [`Error::ResourceLocked`].
    pub fn check(&self, resource: &ResourceName, session: u64) -> Result<()> {
        if self.with(resource, |t| t.blocks(session)) {
            Err(Error::ResourceLocked)
        } else {
            Ok(())
        }
    }

    /// [`LockTable::try_lock`] on `resource`'s table.
    pub fn try_lock(&self, resource: &ResourceName, session: u64, kind: &LockKind) -> bool {
        self.with(resource, |t| t.try_lock(session, kind))
    }

    /// [`LockTable::unlock`] on `resource`'s table.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `session` holds no lock.
    pub fn unlock(&self, resource: &ResourceName, session: u64) -> Result<()> {
        if self.with(resource, |t| t.unlock(session)) {
            Ok(())
        } else {
            Err(Error::InvalidArgument("the session holds no lock".into()))
        }
    }

    /// [`LockTable::release_all`] on `resource`'s table.
    pub fn release_all(&self, resource: &ResourceName, session: u64) {
        self.with(resource, |t| t.release_all(session));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(key: &str) -> LockKind {
        LockKind::Shared { key: key.into() }
    }

    #[test]
    fn exclusive_nests() {
        let mut t = LockTable::new();
        assert!(t.try_lock(1, &LockKind::Exclusive));
        assert!(t.try_lock(1, &LockKind::Exclusive));
        assert!(t.unlock(1));
        assert!(t.blocks(2), "still held once");
        assert!(t.unlock(1));
        assert!(!t.blocks(2));
        assert!(!t.unlock(1), "nothing left to unlock");
    }

    #[test]
    fn shared_by_key() {
        let mut t = LockTable::new();
        assert!(t.try_lock(1, &shared("k")));
        assert!(t.try_lock(2, &shared("k")));
        assert!(!t.try_lock(3, &shared("other")));
        assert!(t.blocks(3) && !t.blocks(1) && !t.blocks(2));
        assert!(!t.try_lock(3, &LockKind::Exclusive), "others share it");
        t.release_all(2);
        assert!(t.try_lock(1, &LockKind::Exclusive), "sole holder upgrades");
        t.release_all(1);
        assert_eq!(t, LockTable::new());
    }

    #[test]
    fn exclusive_keeps_shared_out() {
        let mut t = LockTable::new();
        assert!(t.try_lock(1, &LockKind::Exclusive));
        assert!(!t.try_lock(2, &shared("k")));
        assert!(t.try_lock(1, &shared("k")), "the owner may also share");
    }
}
