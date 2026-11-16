fn overtone_music(game: &crate::game::Game<'_>, music: &mut Option<horizon_client::music::Music>, volume: &mut f32) {
    crate::game::overtone_lyrics::on_music(game, music, volume);
}
