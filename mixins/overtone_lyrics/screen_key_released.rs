fn overtone_screen_key_released(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, key: &str, modifiers: u32, canceled: &mut bool) {
    crate::game::overtone_lyrics::on_screen_key_released(game, screen, key, modifiers, canceled);
}
