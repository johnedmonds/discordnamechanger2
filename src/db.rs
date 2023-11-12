use std::fmt::Display;

use log::{info, warn};
use serenity::model::prelude::{GuildId, Member, UserId};
use sled::{Batch, IVec, Tree};

pub trait BatchAddable {
    fn add_to_batch(&self, batch: &mut Batch);
}
impl<S: AsRef<str>> BatchAddable for &(UserId, S) {
    fn add_to_batch(&self, batch: &mut Batch) {
        info!("Adding hardcoded {}", self.1.as_ref());
        (DbKey::from(self.0), self.1.as_ref()).add_to_batch(batch);
    }
}
impl<S: AsRef<str>> BatchAddable for (DbKey, S) {
    fn add_to_batch(&self, batch: &mut Batch) {
        info!("Adding from key {}", self.1.as_ref());
        batch.insert(IVec::from(self.0.as_ref()), self.1.as_ref());
    }
}
impl<'a> BatchAddable for &'a Member {
    fn add_to_batch(&self, batch: &mut Batch) {
        info!("Adding member {}", self.display_name());
        (&(self.user.id, self.display_name().as_str())).add_to_batch(batch);
    }
}

#[derive(Clone, Copy)]
pub struct DbKey([u8; 8]);
impl From<UserId> for DbKey {
    fn from(value: UserId) -> Self {
        Self(value.0.to_be_bytes())
    }
}
impl From<GuildId> for DbKey {
    fn from(value: GuildId) -> Self {
        Self(value.0.to_be_bytes())
    }
}
impl AsRef<[u8]> for DbKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Display for DbKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", u64::from_be_bytes(self.0))
    }
}

pub fn make_name_batch<T: BatchAddable, I: Iterator<Item = T>>(members: I) -> Batch {
    let mut batch = Batch::default();
    for member in members {
        member.add_to_batch(&mut batch);
    }
    batch
}
pub fn has_overridden_name(member: &Member, name_overrides: &Tree) -> bool {
    info!(
        "Checking {} against {}",
        get_name(name_overrides, DbKey::from(member.user.id)).unwrap_or("".to_string()),
        member.display_name()
    );
    get_name(name_overrides, DbKey::from(member.user.id)).as_ref()
        == Some(member.display_name().as_ref())
}
pub fn name_overrides_db_tree_name(guild_id: GuildId) -> [u8; 9] {
    let mut name = [b'o'; 9];
    name[1..].copy_from_slice(&DbKey::from(guild_id).0);
    name
}
pub fn get_name(tree: &Tree, user_id: DbKey) -> Option<String> {
    match tree.get(user_id) {
        Err(e) => {
            warn!("Failed to get name for {user_id}: {e}");
            None
        }
        Ok(value) => match String::from_utf8(value?.as_ref().to_vec()) {
            Err(e) => {
                warn!("Corrupt name for {user_id}: {e}");
                None
            }
            Ok(name) => Some(name.to_string()),
        },
    }
}
