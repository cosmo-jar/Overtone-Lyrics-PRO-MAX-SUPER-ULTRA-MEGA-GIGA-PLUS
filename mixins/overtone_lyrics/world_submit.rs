fn overtone_world_submit(game: &crate::game::Game<'_>, draw: &mut crate::game::native_world_draw::NativeWorldDraw) {
    crate::game::overtone_lyrics::on_world_submit(game, draw);
}
