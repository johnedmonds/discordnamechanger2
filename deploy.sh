cargo build --release
docker build . -t us-east1-docker.pkg.dev/discord-name-changer/docker/discordnamechanger
docker push us-east1-docker.pkg.dev/discord-name-changer/docker/discordnamechanger