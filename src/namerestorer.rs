use futures::{stream::iter, StreamExt};
use itertools::{Either, Itertools};
use log::{debug, info, warn};
use serenity::{
    all::EditMember,
    http::Http,
    model::prelude::{GuildId, UserId},
};
use sled::{Batch, Db};

use crate::db::{get_name, name_overrides_db_tree_name, DbKey, NameOverridesDbTreeNameType};

pub async fn restore_overridden(token: String, db: Db) {
    struct OverriddenUserName {
        user_id: UserId,
        guild_id: GuildId,
        original_name: String,
        overridden_name: String,
    }
    let http = Http::new(&token);
    let overridden_names = db
        .tree_names()
        .into_iter()
        .filter_map(|name| -> Option<NameOverridesDbTreeNameType> { name.as_ref().try_into().ok() })
        .flat_map(|name| {
            let [_, key @ ..] = name;
            let guild_id_db_key = DbKey(key);
            let guild_id: GuildId = guild_id_db_key.clone().into();
            let names = db.open_tree(guild_id_db_key.as_ref()).unwrap();
            let name_overrides = db.open_tree(name).unwrap();
            name_overrides.into_iter().map(move |result| {
                let (key, value) = result.unwrap();
                let user_id = DbKey(key.as_ref().try_into().unwrap());
                OverriddenUserName {
                    guild_id,
                    user_id: user_id.into(),
                    original_name: get_name(&names, user_id).unwrap(),
                    overridden_name: String::from_utf8(value.to_vec()).unwrap(),
                }
            })
        });
    futures::stream::iter(overridden_names)
        .map(
            |OverriddenUserName {
                 guild_id,
                 user_id,
                 original_name,
                 overridden_name,
             }| {
                let http = &http;
                async move {
                    if http
                        .get_member(guild_id, user_id)
                        .await
                        .map(|member| member.display_name() == overridden_name.as_str())
                        .unwrap_or(false)
                    {
                        info!("Attempting to replace {overridden_name} with {original_name} to {user_id}");
                        match guild_id
                            .edit_member(http, user_id, EditMember::new().nickname(original_name))
                            .await
                        {
                            Err(e) => {
                                warn!("Failed to update {user_id} {e}");
                                None
                            },
                            Ok(_) => Some((guild_id, user_id))
                        }
                    } else {
                        Some((guild_id, user_id))
                    }
                }
            },
        )
        .buffer_unordered(10)
        .flat_map(futures::stream::iter)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .into_grouping_map()
        .fold(Batch::default(), |mut batch, _key, value| {
            batch.remove(DbKey::from(value).as_ref());
            batch
        })
        .into_iter()
        .for_each(|(guild_id, batch)| {
            db.open_tree(name_overrides_db_tree_name(guild_id))
                .unwrap()
                .apply_batch(batch)
                .unwrap()
        });
}

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
                .edit_member(http, user_id, EditMember::new().nickname(&name))
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
