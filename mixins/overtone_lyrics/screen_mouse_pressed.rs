fn overtone_screen_mouse_pressed(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, button: i32, modifiers: u32, canceled: &mut bool) {
    crate::game::overtone_lyrics::on_screen_mouse_pressed(game, screen, button, modifiers, canceled);
}
