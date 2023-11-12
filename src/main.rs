use clap::{Parser, Subcommand};
use simple_logger::SimpleLogger;

mod db;
mod namechanger;
mod namerestorer;

#[derive(Subcommand)]
enum Commands {
    Restore,
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
            Commands::Restore => namerestorer::run(token, db).await,
        },
        None => namechanger::run(token, db).await,
    }
}
