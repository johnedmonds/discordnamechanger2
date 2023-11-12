use std::{borrow::Cow, collections::HashMap, fmt::Display, pin::pin};

use futures::{join, stream::iter, StreamExt};
use log::{debug, info, warn};

use serenity::{
    async_trait,
    client::Cache,
    framework::StandardFramework,
    model::{
        gateway::Activity,
        prelude::{
            ActivityType, Channel, ChannelId, ChannelType, Guild, GuildChannel, GuildId, Member,
            Presence, UserId,
        },
        user::User,
        voice::VoiceState,
    },
    prelude::*,
};

use sled::Db;

use crate::db::{
    get_name, has_overridden_name, make_name_batch, name_overrides_db_tree_name, DbKey,
};

fn current_champion_from_activities<'a, I: IntoIterator<Item = &'a Activity>>(
    activities: I,
) -> Option<&'a str> {
    activities
        .into_iter()
        .inspect(|activity| debug!("Checking activity {activity:?}"))
        .flat_map(|activity: &Activity| {
            let is_valid_activity =
                activity.kind == ActivityType::Playing && activity.name == "League of Legends";
            is_valid_activity.then_some(activity.assets.as_ref()?.large_text.as_ref()?)
        })
        .next()
        .map(String::as_str)
}
struct Handler {
    db: Db,
}

fn gen_derangement(size: usize) -> Vec<usize> {
    match size {
        0 => vec![],
        1 => vec![0],
        _ => {
            let mut rng = rand::thread_rng();
            derangement::derange::Derange::new(&mut rng, size)
                .map()
                .to_vec()
        }
    }
}

async fn set_nicks<'a, S: ToString + Display, I: IntoIterator<Item = (UserId, S)>>(
    ctx: &Context,
    guild_id: GuildId,
    nicks: I,
) {
    iter(nicks.into_iter())
        .for_each_concurrent(10, |(user_id, nick)| async move {
            info!("Setting nickname to {nick} for {user_id}");
            if let Err(e) = guild_id
                .edit_member(&ctx.http, user_id, move |m| m.nickname(nick))
                .await
            {
                warn!("Failed to set nickname for {user_id}: {e:?}");
            }
        })
        .await;
}
async fn channel_members(cache: &Cache, channel_id: ChannelId) -> Option<Vec<Member>> {
    match channel_id
        .to_channel_cached(cache)?
        .guild()?
        .members(cache)
        .await
    {
        Ok(members) => Some(members),
        Err(e) => {
            warn!("Error fetching members of {channel_id}: {e:?}");
            None
        }
    }
}
fn get_guild_voice_channels(
    guild_channels: HashMap<ChannelId, Channel>,
) -> impl Iterator<Item = GuildChannel> + 'static {
    guild_channels
        .into_values()
        .flat_map(|channel| channel.guild())
        .filter(|channel| channel.kind == ChannelType::Voice)
}

#[async_trait]
impl EventHandler for Handler {
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: bool) {
        info!("Guild create for {} ({})", guild.name, guild.id);
        let names = self.db.open_tree(DbKey::from(guild.id)).unwrap();
        let name_overrides = self
            .db
            .open_tree(name_overrides_db_tree_name(guild.id))
            .unwrap();
        names
            .apply_batch(make_name_batch(
                guild
                    .members
                    .values()
                    .filter(|member| !has_overridden_name(member, &name_overrides)),
            ))
            .unwrap();
        let voice_channels = get_guild_voice_channels(guild.channels);
        iter(voice_channels)
            .for_each_concurrent(10, |channel| {
                info!(
                    "Examining channel {} ({}) in {} ({})",
                    channel.name, channel.id, guild.name, guild.id
                );
                self.sync_nicks(&ctx, guild.id, channel.id)
            })
            .await;
    }

    async fn presence_update(&self, ctx: Context, presence: Presence) {
        async fn find_channel_containing_user(
            presence: Presence,
            cache: &Cache,
        ) -> Option<ChannelId> {
            let voice_channels =
                get_guild_voice_channels(presence.guild_id?.to_guild_cached(cache)?.channels);
            let channel_with_user = iter(voice_channels).filter_map(|channel| async move {
                channel
                    .members(cache)
                    .await
                    .into_iter()
                    .any(|member| {
                        member
                            .into_iter()
                            .any(|member| member.user.id == presence.user.id)
                    })
                    .then_some(channel.id)
            });
            let mut channel_with_user = pin!(channel_with_user);
            channel_with_user.next().await
        }
        if let Some(guild_id) = presence.guild_id {
            if let Some(channel_id) = find_channel_containing_user(presence, &ctx.cache).await {
                self.sync_nicks(&ctx, guild_id, channel_id).await;
            }
        }
    }

    async fn voice_state_update(
        &self,
        ctx: Context,
        old_state: Option<VoiceState>,
        new_state: VoiceState,
    ) {
        let new_state_future = self.process_voice_state_update(&ctx, &new_state);
        let old_state_future = async {
            if let Some(voice_state) = old_state {
                let restore_leaving_user_name_future = async {
                    if let Some(ref member) = voice_state.member {
                        let names = self.db.open_tree(DbKey::from(member.guild_id)).unwrap();
                        let nick_to_restore = get_name(&names, DbKey::from(member.user.id))
                            .map(Cow::Owned)
                            .unwrap_or(Cow::Borrowed(&member.user.name));
                        info!(
                            "Restoring nickname {nick_to_restore} to {} ({})",
                            member.user.name, member.user.id
                        );
                        if let Err(e) = member
                            .guild_id
                            .edit_member(&ctx.http, member.user.id, |m| {
                                m.nickname(nick_to_restore.clone())
                            })
                            .await
                        {
                            warn!(
                                "Failed to restore user name {nick_to_restore} to {} ({}): {e:?}",
                                member.user.name, member.user.id
                            );
                        }
                    }
                };
                join!(
                    restore_leaving_user_name_future,
                    self.process_voice_state_update(&ctx, &voice_state),
                );
            }
        };
        join!(new_state_future, old_state_future);
    }

    async fn guild_member_update(
        &self,
        _ctx: Context,
        _old_if_available: Option<Member>,
        new: Member,
    ) {
        let name_overrides = self
            .db
            .open_tree(name_overrides_db_tree_name(new.guild_id))
            .unwrap();
        if !has_overridden_name(&new, &name_overrides) {
            let user_id_key = DbKey::from(new.user.id);
            name_overrides.remove(user_id_key).unwrap();
            let names = self.db.open_tree(DbKey::from(new.guild_id)).unwrap();
            names
                .apply_batch(make_name_batch(std::iter::once((
                    user_id_key,
                    new.display_name().as_str(),
                ))))
                .unwrap();
        }
    }
    async fn guild_member_addition(&self, _ctx: Context, new_member: Member) {
        self.db
            .open_tree(DbKey::from(new_member.guild_id))
            .unwrap()
            .insert(
                DbKey::from(new_member.user.id),
                new_member.display_name().as_str(),
            )
            .unwrap();
    }
    async fn guild_member_removal(
        &self,
        _ctx: Context,
        guild_id: GuildId,
        user: User,
        _member_data_if_available: Option<Member>,
    ) {
        let key = DbKey::from(user.id);
        self.db
            .open_tree(name_overrides_db_tree_name(guild_id))
            .unwrap()
            .remove(key)
            .unwrap();
        self.db
            .open_tree(DbKey::from(guild_id))
            .unwrap()
            .remove(key)
            .unwrap();
    }
}
impl Handler {
    async fn process_voice_state_update(&self, ctx: &Context, voice_state: &VoiceState) {
        if let Some(guild_id) = voice_state.guild_id {
            if let Some(channel_id) = voice_state.channel_id {
                self.sync_nicks(ctx, guild_id, channel_id).await;
            }
        }
    }
    async fn sync_nicks(&self, ctx: &Context, guild_id: GuildId, channel_id: ChannelId) {
        info!("Syncing nicknames for channel {channel_id} in guild {guild_id}");
        let members = channel_members(&ctx.cache, channel_id)
            .await
            .unwrap_or(vec![]);
        let derangement = gen_derangement(members.len());
        if let Some(guild) = guild_id.to_guild_cached(&ctx.cache) {
            let names = self.db.open_tree(DbKey::from(guild_id)).unwrap();
            let new_nicks:Vec<_> = members.iter().enumerate().map(|(user_id_index, member)| {
                let from_user = &members[derangement[user_id_index]].user;
                let source_champion_named = guild.presences.get(&from_user.id).and_then(|presence|current_champion_from_activities(&presence.activities));
                let new_nick = if let Some(champion) = source_champion_named {
                    info!(
                        "Selected champion {champion} (from {} ({}) as nick for {} ({})",
                        from_user.name, from_user.id, member.user.name, member.user.id
                    );
                    Cow::Borrowed(champion)
                } else if let Some(nick) = get_name(&names, DbKey::from(member.user.id) ){
                    info!("Could not determine champion for {} ({}). Selected hardcoded nick {nick} for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                    Cow::Owned(nick)
                } else {
                    info!("Could not determine champion for {} ({}). Selected username for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                    Cow::Borrowed(member.user.name.as_str())
                };
                (member.user.id, new_nick)
            }).collect();
            // First set to the old nicks so that if we crash, the old nick will stick.
            let old_nicks: Vec<_> = members
                .iter()
                .flat_map(|member| {
                    Some((
                        member.user.id,
                        get_name(&names, DbKey::from(member.user.id))?,
                    ))
                })
                .collect();
            set_nicks(ctx, guild_id, old_nicks).await;
            let name_overrides = self
                .db
                .open_tree(name_overrides_db_tree_name(guild_id))
                .unwrap();
            // Clear and set the overrides. We want to record the overrides before we actually make the change just in case we crash in the middle.
            name_overrides.clear().unwrap();
            name_overrides
                .apply_batch(make_name_batch(new_nicks.iter()))
                .unwrap();
            set_nicks(ctx, guild_id, new_nicks).await;
        } else {
            warn!("Failed to sync nicknames for guild {guild_id} because the guild wasn't found in the cache");
        }
    }
}

pub async fn run(token: String, db: Db) {
    let intents = GatewayIntents::GUILD_PRESENCES
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler { db })
        .framework(StandardFramework::default())
        .await
        .expect("Error creating client");

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}
