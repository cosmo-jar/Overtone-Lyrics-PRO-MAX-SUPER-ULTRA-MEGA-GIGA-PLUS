fn overtone_screen_key_pressed(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, key: &str, repeat: bool, modifiers: u32, canceled: &mut bool) {
    crate::game::overtone_lyrics::on_screen_key_pressed(game, screen, key, repeat, modifiers, canceled);
}
