fn overtone_settings(game: &mut crate::game::Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    crate::game::overtone_lyrics::on_settings(game, mod_id, buttons);
}
