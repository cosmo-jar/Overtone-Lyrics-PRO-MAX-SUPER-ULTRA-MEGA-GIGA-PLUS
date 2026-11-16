fn overtone_drop_file(game: &mut crate::game::Game<'_>, path: &std::path::Path) {
    crate::game::overtone_lyrics::on_drop_file(game, path);
}
