fn overtone_key(game: &mut crate::game::Game<'_>, key: &str, pressed: bool) {
    crate::game::overtone_lyrics::on_key(game, key, pressed);
}
