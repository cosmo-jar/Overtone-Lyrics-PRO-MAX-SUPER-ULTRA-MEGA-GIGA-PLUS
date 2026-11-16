fn overtone_input_key(game: &mut crate::game::Game<'_>, event: &winit::event::KeyEvent, modifiers: u32) {
    crate::game::overtone_lyrics::on_input_key(game, event, modifiers);
}
