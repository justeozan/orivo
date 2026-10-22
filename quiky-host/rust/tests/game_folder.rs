use quiky_host::download::detect_game_folder;

#[test]
fn detect_stick_it_folder() {
    let folders = vec![
        "Other Game".to_string(),
        "Stick It to the Stickman [FitGirl Repack]".to_string(),
    ];
    assert_eq!(
        detect_game_folder(&folders, "Stick It to the Stickman"),
        Some("Stick It to the Stickman [FitGirl Repack]".to_string())
    );
}
