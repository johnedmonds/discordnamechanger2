mod namechanger;

#[tokio::main]
async fn main() {
    namechanger::run().await;
}
