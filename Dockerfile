FROM debian:bookworm-20230703-slim
COPY target/release/discordnamechanger /discordnamechanger
CMD /discordnamechanger