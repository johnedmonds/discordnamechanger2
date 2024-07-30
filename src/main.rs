use clap::{Parser, Subcommand};
use db::DbKey;
use serenity::model::id::{GuildId, UserId};
use simple_logger::SimpleLogger;

mod db;
mod namechanger;
mod namerestorer;

#[derive(Subcommand)]
enum Commands {
    Restore {
        #[arg(short, long)]
        overridden_only: bool,
    },
    Set {
        #[arg(short)]
        guild_id: u64,
        #[arg(short)]
        user_id: u64,
        #[arg(short)]
        name: String,
    },
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let token = std::fs::read_to_string("token.txt").unwrap();
    SimpleLogger::default()
        .with_level(log::LevelFilter::Warn)
        .with_module_level("discordnamechanger", log::LevelFilter::Debug)
        .init()
        .unwrap();
    let db = sled::open("names.sled.db").unwrap();

    match cli.command {
        Some(command) => match command {
            Commands::Restore { overridden_only } => {
                if overridden_only {
                    namerestorer::restore_overridden(token, db).await
                } else {
                    namerestorer::run(token, db).await
                }
            }
            Commands::Set {
                guild_id,
                user_id,
                name,
            } => {
                db.open_tree(DbKey::from(GuildId::new(guild_id)))
                    .unwrap()
                    .insert(DbKey::from(UserId::new(user_id)), name.as_str())
                    .unwrap();
            }
        },
        None => namechanger::run(token, db).await,
    }
}
