fn overtone_wheel(game: &mut crate::game::Game<'_>, steps: f32, claimed: &mut bool) {
    crate::game::overtone_lyrics::on_wheel(game, steps, claimed);
}
