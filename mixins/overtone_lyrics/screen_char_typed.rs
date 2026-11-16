fn overtone_screen_char_typed(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, ch: char, modifiers: u32, canceled: &mut bool) {
    crate::game::overtone_lyrics::on_screen_char_typed(game, screen, ch, modifiers, canceled);
}
