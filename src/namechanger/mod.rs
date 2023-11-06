use std::{collections::HashMap, pin::pin};

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
            Presence,
        },
        voice::VoiceState,
    },
    prelude::*,
};
use simple_logger::SimpleLogger;

use nick_overrides::nick_override;

mod nick_overrides;

fn current_champion_from_activities<'a, I: IntoIterator<Item = &'a Activity>>(
    activities: I,
) -> Option<&'a str> {
    activities
        .into_iter()
        .inspect(|activity| debug!("Checking activity {activity:?}"))
        .flat_map(|activity: &Activity| {
            let is_valid_activity = activity.kind == ActivityType::Playing
                && activity.name == "League of Legends";
            is_valid_activity.then_some(activity.assets.as_ref()?.large_text.as_ref()?)
        })
        .next()
        .map(String::as_str)
}
struct Handler;

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

async fn sync_nicks(ctx: &Context, guild_id: GuildId, channel_id: ChannelId) {
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
    info!("Syncing nicknames for channel {channel_id} in guild {guild_id}");
    let members = channel_members(&ctx.cache, channel_id)
        .await
        .unwrap_or(vec![]);
    let derangement = gen_derangement(members.len());
    if let Some(guild) = guild_id.to_guild_cached(&ctx.cache) {
        let user_nicks = members.iter().enumerate().map(|(user_id_index, member)| {
            let from_user = &members[derangement[user_id_index]].user;
            let new_nick = if let Some(presence) = guild.presences.get(&from_user.id) {
                if let Some(champion) = current_champion_from_activities(&presence.activities) {
                    info!(
                        "Selected champion {champion} (from {} ({}) as nick for {} ({})",
                        from_user.name, from_user.id, member.user.name, member.user.id
                    );
                    champion
                } else if let Some(nick) = nick_override(member.user.id) {
                    info!("Could not determine champion for {} ({}). Selected hardcoded nick {nick} for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                    nick
                } else {
                    info!("Could not determine champion for {} ({}). Selected username for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                    &member.user.name
                }
            } else if let Some(nick) = nick_override(member.user.id) {
                info!("{} ({}) not in game. Selected hardcoded nick {nick} for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                nick
            } else {
                info!("{} ({}) not in game. Selected username for {} ({})", from_user.name, from_user.id, member.user.name, member.user.id);
                &member.user.name
            };
            (member.user.id, new_nick)
        });
        iter(user_nicks)
            .for_each_concurrent(10, |(user_id, nick)| async move {
                info!("Setting nickname to {nick} for {user_id}");
                if let Err(e) = guild_id
                    .edit_member(&ctx.http, user_id, move |m| m.nickname(nick))
                    .await
                {
                    warn!("Failed to set nickname to {nick} for {user_id}: {e:?}");
                }
            })
            .await;
    } else {
        warn!("Failed to sync nicknames for guild {guild_id} because the guild wasn't found in the cache");
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
        let voice_channels = get_guild_voice_channels(guild.channels);
        iter(voice_channels)
            .for_each_concurrent(10, |channel| {
                info!(
                    "Examining channel {} ({}) in {} ({})",
                    channel.name, channel.id, guild.name, guild.id
                );
                sync_nicks(&ctx, guild.id, channel.id)
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
                sync_nicks(&ctx, guild_id, channel_id).await;
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
                        let nick_to_restore =
                            nick_override(member.user.id).unwrap_or(&member.user.name);
                        info!(
                            "Restoring nickname {nick_to_restore} to {} ({})",
                            member.user.name, member.user.id
                        );
                        if let Err(e) = member
                            .guild_id
                            .edit_member(&ctx.http, member.user.id, |m| m.nickname(nick_to_restore))
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
}
impl Handler {
    async fn process_voice_state_update(&self, ctx: &Context, voice_state: &VoiceState) {
        if let Some(guild_id) = voice_state.guild_id {
            if let Some(channel_id) = voice_state.channel_id {
                sync_nicks(ctx, guild_id, channel_id).await;
            }
        }
    }
}

pub async fn run() {
    let token = std::fs::read_to_string("token.txt").unwrap();
    SimpleLogger::default()
        .with_level(log::LevelFilter::Warn)
        .with_module_level("discordnamechanger", log::LevelFilter::Debug)
        .init()
        .unwrap();
    let intents = GatewayIntents::GUILD_PRESENCES
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILDS;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .framework(StandardFramework::default())
        .await
        .expect("Error creating client");

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}
