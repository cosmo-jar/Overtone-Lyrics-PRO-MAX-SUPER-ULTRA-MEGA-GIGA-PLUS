fn overtone_gui_render_post(game: &crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch, gui_hidden: bool, focused: bool) {
    crate::game::overtone_lyrics::on_gui_render_post(game, batch, gui_hidden, focused);
}
