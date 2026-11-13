//! Yjs' `PermanentUserData`: which user each client ID belongs to and which deletions each user
//! made, kept in the document itself so that every peer can show who wrote or deleted what.
//!
//! The data lives in a map (Yjs uses `doc.getMap('users')`) with one entry per user:
//! `{ ids: [client IDs], ds: [encoded delete sets] }`. yrs writes and reads the same layout, so
//! Yjs and yrs peers share it.
//!
//! Yjs records each local transaction's deletions from a timer after the transaction ends. yrs
//! has no timers, so recording is explicit: call [PermanentUserData::record_deletions] (or
//! [PermanentUserData::record_local_deletions] at the end of a transaction). Deleted content is
//! only worth attributing if it is kept, so pair this with
//! [Options::gc_filter](crate::Options::gc_filter) or `skip_gc`.

use crate::block::ClientID;
use crate::updates::decoder::Decode;
use crate::updates::encoder::Encode;
use crate::{
    Any, Array, ArrayPrelim, ArrayRef, IdSet, Map, MapPrelim, MapRef, Out, ReadTxn, TransactionMut,
    ID,
};
use std::convert::TryFrom;

const IDS: &str = "ids";
const DS: &str = "ds";

/// See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct PermanentUserData {
    users: MapRef,
}

impl PermanentUserData {
    /// Keeps the user data in `users`. Yjs' default is the root map `"users"`.
    pub fn new(users: MapRef) -> Self {
        PermanentUserData { users }
    }

    /// The map the user data is kept in.
    pub fn users(&self) -> &MapRef {
        &self.users
    }

    /// Records that `client` belongs to `user`, like Yjs' `setUserMapping`. Recording the same
    /// pair again does nothing.
    ///
    /// If two peers create the entry of the same new user at the same time, one entry wins and
    /// the other's client IDs are lost. Yjs copies them over from an observer; with yrs, call
    /// this again for the local client after syncing.
    pub fn set_user_mapping(&self, txn: &mut TransactionMut, client: ClientID, user: &str) {
        let ids = self.user_array(txn, user, IDS);
        let known = ids.iter(txn).any(|v| client_of(&v) == Some(client.get()));
        if !known {
            ids.push_back(txn, Any::from(client.get() as i64));
        }
    }

    /// Records that `user` made `deletions`. Empty sets are not recorded.
    pub fn record_deletions(&self, txn: &mut TransactionMut, user: &str, deletions: &IdSet) {
        if deletions.is_empty() {
            return;
        }
        let ds = self.user_array(txn, user, DS);
        ds.push_back(txn, Any::from(deletions.encode_v1()));
    }

    /// Records the deletions made so far in `txn` as `user`'s. Call it last in a local
    /// transaction: deletions made after it are not recorded.
    pub fn record_local_deletions(&self, txn: &mut TransactionMut, user: &str) {
        let deletions = txn.delete_set().clone();
        self.record_deletions(txn, user, &deletions);
    }

    /// The user `client` belongs to, like Yjs' `getUserByClientId`.
    pub fn user_by_client_id<T: ReadTxn>(&self, txn: &T, client: ClientID) -> Option<String> {
        self.users.iter(txn).find_map(|(user, entry)| {
            let ids = nested_array(txn, &entry, IDS)?;
            let found = ids.iter(txn).any(|v| client_of(&v) == Some(client.get()));
            found.then(|| user.to_string())
        })
    }

    /// The user who deleted `id`, like Yjs' `getUserByDeletedId`. Entries that don't decode are
    /// skipped.
    pub fn user_by_deleted_id<T: ReadTxn>(&self, txn: &T, id: &ID) -> Option<String> {
        self.users.iter(txn).find_map(|(user, entry)| {
            let ds = nested_array(txn, &entry, DS)?;
            let found = ds.iter(txn).any(|v| match v {
                Out::Any(Any::Buffer(bytes)) => {
                    IdSet::decode_v1(&bytes).is_ok_and(|set| set.contains(id))
                }
                _ => false,
            });
            found.then(|| user.to_string())
        })
    }

    /// The `ids` or `ds` array of `user`, creating the user's entry if needed.
    fn user_array(&self, txn: &mut TransactionMut, user: &str, key: &str) -> ArrayRef {
        let entry = match self.users.get(txn, user) {
            Some(Out::YMap(entry)) => entry,
            _ => self.users.insert(
                txn,
                user,
                MapPrelim::from([(IDS, ArrayPrelim::default()), (DS, ArrayPrelim::default())]),
            ),
        };
        match entry.get(txn, key) {
            Some(Out::YArray(array)) => array,
            _ => entry.insert(txn, key, ArrayPrelim::default()),
        }
    }
}

fn nested_array<T: ReadTxn>(txn: &T, entry: &Out, key: &str) -> Option<ArrayRef> {
    match entry {
        Out::YMap(map) => match map.get(txn, key)? {
            Out::YArray(array) => Some(array),
            _ => None,
        },
        _ => None,
    }
}

/// Yjs writes client IDs as numbers, which arrive as integers or floats.
fn client_of(value: &Out) -> Option<u64> {
    match value {
        Out::Any(Any::Number(n)) => n.as_i64().and_then(|n| u64::try_from(n).ok()),
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use crate::updates::decoder::Decode;
    use crate::{
        Array, ClientID, Doc, Map, Options, PermanentUserData, ReadTxn, Text, Transact, Update, ID,
    };

    fn doc(client: u64) -> Doc {
        Doc::with_options(Options {
            skip_gc: true,
            ..Options::with_client_id(ClientID::new(client))
        })
    }

    fn sync(from: &Doc, to: &Doc) {
        let update = from
            .transact()
            .encode_state_as_update_v1(&to.transact().state_vector());
        to.transact_mut()
            .apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
    }

    #[test]
    fn deletions_and_clients_are_attributed_to_their_users_on_every_peer() {
        let alice = doc(1);
        let bob = doc(2);
        let pud = PermanentUserData::new(alice.get_or_insert_map("users"));
        let text = alice.get_or_insert_text("text");
        {
            let mut txn = alice.transact_mut();
            text.insert(&mut txn, 0, "hello"); // clocks 0..5
            pud.set_user_mapping(&mut txn, ClientID::new(1), "alice");
            pud.set_user_mapping(&mut txn, ClientID::new(1), "alice");
        }
        {
            let mut txn = alice.transact_mut();
            text.remove_range(&mut txn, 0, 2);
            pud.record_local_deletions(&mut txn, "alice");
        }
        {
            // Nothing deleted: nothing recorded.
            let mut txn = alice.transact_mut();
            pud.record_local_deletions(&mut txn, "alice");
        }

        sync(&alice, &bob);
        let pud_b = PermanentUserData::new(bob.get_or_insert_map("users"));
        pud_b.set_user_mapping(&mut bob.transact_mut(), ClientID::new(2), "bob");
        sync(&bob, &alice);

        for (doc, pud) in [(&alice, &pud), (&bob, &pud_b)] {
            let txn = doc.transact();
            let alice_id = ID::new(ClientID::new(1), 0);
            assert_eq!(
                pud.user_by_client_id(&txn, ClientID::new(1)).as_deref(),
                Some("alice")
            );
            assert_eq!(
                pud.user_by_client_id(&txn, ClientID::new(2)).as_deref(),
                Some("bob")
            );
            assert_eq!(pud.user_by_client_id(&txn, ClientID::new(3)), None);
            assert_eq!(
                pud.user_by_deleted_id(&txn, &alice_id).as_deref(),
                Some("alice")
            );
            let kept = ID::new(ClientID::new(1), 4);
            assert_eq!(pud.user_by_deleted_id(&txn, &kept), None);

            let users = pud.users();
            let entry = match users.get(&txn, "alice") {
                Some(crate::Out::YMap(m)) => m,
                other => panic!("{:?}", other),
            };
            let len = |key| match entry.get(&txn, key) {
                Some(crate::Out::YArray(a)) => a.len(&txn),
                other => panic!("{:?}", other),
            };
            assert_eq!(len("ids"), 1, "a repeated mapping is stored once");
            assert_eq!(len("ds"), 1, "an empty delete set is not stored");
        }
    }
}
