fn overtone_frame(game: &mut crate::game::Game<'_>, elapsed: std::time::Duration) {
    crate::game::overtone_lyrics::on_frame(game, elapsed);
}
