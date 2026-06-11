use spawn_ecs::{Resource, World};
use spawn_engine::{App, Style, UiTree};

#[derive(Default)]
struct Score(u32);
impl Resource for Score {}

#[test]
fn ui_setup_builds_a_tree_and_updates_set_text_each_tick() {
    let mut app = App::new();
    app.insert_resource(Score(7));

    app.add_ui_setup(|tree: &mut UiTree| {
        let root = tree.root();
        tree.create_node(Style::default(), root)?;
        Ok(())
    });

    app.add_ui_update(|world: &World, tree: &mut UiTree| {
        let root = tree.root();
        if let Some(label) = tree.children(root).and_then(|c| c.first().copied()) {
            let score = world.get_resource::<Score>().map(|s| s.0).unwrap_or(0);
            tree.set_text(label, Some(format!("Score {score}")))?;
        }
        Ok(())
    });

    let mut engine = app.build_headless().unwrap();
    assert!(
        engine.ui().is_some(),
        "a tree was built from the setup hook"
    );

    engine.tick().unwrap();

    let ui = engine.ui().unwrap();
    let label = ui
        .children(ui.root())
        .and_then(|c| c.first().copied())
        .expect("the setup hook created a child node");
    assert_eq!(ui.text(label), Some("Score 7"));
}
