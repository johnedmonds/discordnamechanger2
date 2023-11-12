use futures::{stream::iter, StreamExt};
use itertools::{Either, Itertools};
use log::{debug, info, warn};
use serenity::{
    http::Http,
    model::prelude::{GuildId, UserId},
};
use sled::Db;

use crate::db::DbKey;

pub async fn run(token: String, db: Db) {
    let http = Http::new(&token);
    let (name_trees, name_override_tree_names): (Vec<_>, Vec<_>) = db
        .tree_names()
        .into_iter()
        .filter(|name| name != &db.name())
        .partition_map(|name| {
            match name
                .as_ref()
                .try_into()
                .map(|key| -> GuildId { DbKey(key).into() })
            {
                Ok(guild_id) => Either::Left((guild_id, db.open_tree(name).unwrap())),
                Err(_) => Either::Right(name),
            }
        });
    let names = name_trees.into_iter().flat_map(|(guild_id, tree)| {
        tree.into_iter().map(move |result| {
            let (key, value) = result.unwrap();
            (
                guild_id.clone(),
                UserId::from(DbKey(key.as_ref().try_into().unwrap())),
                String::from_utf8(value.to_vec()).unwrap(),
            )
        })
    });
    iter(names).for_each_concurrent(10, |(guild_id, user_id, name)| {
        let http=&http;
        async move {
            debug!("Setting user with id {user_id} to name {name} in guild {guild_id}.");
            if let Err(e) = guild_id
                .edit_member(http, user_id, |e| e.nickname(&name))
                .await {
                    warn!("Failed to restore user with id {user_id} to name {name} in guild {guild_id}. {e}");
                }
        }
    }).await;
    for tree_name in name_override_tree_names {
        info!("Dropping {tree_name:?}");
        db.drop_tree(tree_name).unwrap();
    }
}
