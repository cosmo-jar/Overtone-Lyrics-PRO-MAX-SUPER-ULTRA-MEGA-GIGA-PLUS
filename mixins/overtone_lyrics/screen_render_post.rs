fn overtone_screen_render_post(game: &crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch, screen: &horizon_client::mod_api::NativeScreenView<'_>) {
    crate::game::overtone_lyrics::on_screen_render_post(game, batch, screen);
}
