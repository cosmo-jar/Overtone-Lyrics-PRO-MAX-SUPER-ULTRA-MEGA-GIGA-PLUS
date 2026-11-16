fn overtone_screen_mouse_scrolled(game: &mut crate::game::Game<'_>, screen: &str, mouse: [f32; 2], scroll: [f64; 2], modifiers: u32, canceled: &mut bool) {
    crate::game::overtone_lyrics::on_screen_mouse_scrolled(game, screen, mouse, scroll, modifiers, canceled);
}
