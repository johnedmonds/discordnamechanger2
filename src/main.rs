mod namechanger;
mod db;

#[tokio::main]
async fn main() {
    namechanger::run().await;
}
