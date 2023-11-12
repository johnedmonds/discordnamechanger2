use std::{collections::HashMap, fmt::Display, pin::pin};

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
        voice::VoiceState,
    },
    prelude::*,
};
use simple_logger::SimpleLogger;

use nick_overrides::nick_override;
use sled::{Batch, IVec, Tree};

mod nick_overrides;

const NAMES_TREE: &str = "names";
const NAME_OVERRIDES_TREE: &str = "name_overrides";

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
    names: Tree,
    name_overrides: Tree,
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

fn get_name(tree: &Tree, user_id: UserIdDbKey) -> Option<String> {
    match tree.get(user_id.as_key()) {
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

trait BatchAddable {
    fn add_to_batch(&self, batch: &mut Batch);
}
impl<'a> BatchAddable for (UserId, &'a str) {
    fn add_to_batch(&self, batch: &mut Batch) {
        batch.insert(UserIdDbKey(self.0), self.1);
    }
}
impl<'a> BatchAddable for &'a Member {
    fn add_to_batch(&self, batch: &mut Batch) {
        (self.user.id, self.display_name().as_str()).add_to_batch(batch);
    }
}
#[derive(Clone, Copy)]
struct UserIdDbKey(UserId);
impl UserIdDbKey {
    fn as_key(&self) -> [u8; 8] {
        self.0 .0.to_be_bytes()
    }
}
impl Display for UserIdDbKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Into<IVec> for UserIdDbKey {
    fn into(self) -> IVec {
        (&self.as_key()).into()
    }
}

fn make_name_batch<T: BatchAddable, I: Iterator<Item = T>>(members: I) -> Batch {
    let mut batch = Batch::default();
    for member in members {
        member.add_to_batch(&mut batch);
    }
    batch
}
fn has_original_name(member: &Member, name_overrides: &Tree) -> bool {
    get_name(name_overrides, UserIdDbKey(member.user.id)).as_ref()
        == Some(member.display_name().as_ref())
}

#[async_trait]
impl EventHandler for Handler {
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: bool) {
        info!("Guild create for {} ({})", guild.name, guild.id);
        self.names
            .apply_batch(make_name_batch(
                guild
                    .members
                    .values()
                    .filter(|member| has_original_name(member, &self.name_overrides)),
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
            let new_nicks:Vec<_> = members.iter().enumerate().map(|(user_id_index, member)| {
                let from_user = &members[derangement[user_id_index]].user;
                let source_champion_named = guild.presences.get(&from_user.id).and_then(|presence|current_champion_from_activities(&presence.activities));
                let new_nick = if let Some(champion) = source_champion_named {
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
                };
                (member.user.id, new_nick)
            }).collect();
            // First set to the old nicks so that if we crash, the old nick will stick.
            let old_nicks: Vec<_> = members
                .iter()
                .flat_map(|member| {
                    Some((
                        member.user.id,
                        get_name(&self.names, UserIdDbKey(member.user.id))?,
                    ))
                })
                .collect();
            set_nicks(ctx, guild_id, old_nicks).await;
            // Clear and set the overrides. We want to record the overrides before we actually make the change just in case we crash in the middle.
            self.name_overrides.clear().unwrap();
            self.name_overrides
                .apply_batch(make_name_batch(new_nicks.iter().copied()))
                .unwrap();
            set_nicks(ctx, guild_id, new_nicks).await;
        } else {
            warn!("Failed to sync nicknames for guild {guild_id} because the guild wasn't found in the cache");
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

    let db = sled::open("names.sled.db").unwrap();
    let names = db.open_tree(NAMES_TREE).unwrap();
    let name_overrides = db.open_tree(NAME_OVERRIDES_TREE).unwrap();
    let mut client = Client::builder(token, intents)
        .event_handler(Handler {
            names,
            name_overrides,
        })
        .framework(StandardFramework::default())
        .await
        .expect("Error creating client");

    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}
