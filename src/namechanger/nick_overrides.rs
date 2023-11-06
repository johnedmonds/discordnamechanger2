use serenity::model::prelude::UserId;

pub fn nick_override(user_id: UserId) -> Option<&'static str> {
    match user_id.0 {
        1112524192450105346 => Some("Pokemon Go 100 Notifier"),
        454536016728948746 => Some("nilah"),
        183737687713251329 => Some("Jarvan the 1st"),
        170303322488700928 => Some("Nasus"),
        186575396366450688 => Some("Riven"),
        210820678222479371 => Some("seJuani"),
        148944778267066368 => Some("Braum"),
        235839235490316288 => Some("Nidalee"),
        171089305475743746 => Some("Xayah"),
        173466314504011776 => Some("Poppy"),
        279379471604252674 => Some("Nami"),
        341691789994098711 => Some("Mundo--Assistant Physician"),
        184707796791590914 => Some("KEVIN"),
        191381161627484161 => Some("Twitch"),
        117837849830752258 => Some("Jhin"),
        180458252125995008 => Some("Udyr"),
        266978219905908736 => Some("Sett"),
        645968972776472605 => Some("Vayne"),
        244490091932680194 => Some("Sivir"),
        _ => None,
    }
}
