use std::cell::RefCell;
use std::rc::Rc;

use spawn_ecs::World;
use spawn_engine::{App, EngineResult, DEFAULT_FONT};
use spawn_ui::{
    Dimension, Display, FlexDirection, JustifyContent, Label, NodeId, Size, Style, UiTree,
};

use crate::resources::{GameState, Phase};

#[derive(Clone, Copy)]
struct HudNodes {
    score: NodeId,
    lives: NodeId,
    level: NodeId,
    banner: NodeId,
}

struct Hud {
    nodes: Option<HudNodes>,
    last: Option<GameState>,
}

fn bar_style() -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        justify_content: JustifyContent::SpaceBetween,
        size: Size {
            width: Dimension::Percent(1.0),
            height: Dimension::Px(36.0),
        },
        ..Style::default()
    }
}

fn hidden_style() -> Style {
    Style {
        display: Display::None,
        ..Style::default()
    }
}

fn banner_style() -> Style {
    Style {
        justify_content: JustifyContent::Center,
        size: Size {
            width: Dimension::Percent(1.0),
            height: Dimension::Px(48.0),
        },
        ..Style::default()
    }
}

fn build_hud(tree: &mut UiTree) -> EngineResult<HudNodes> {
    let root = tree.root();
    let bar = tree.create_node(bar_style(), root)?;
    let score = Label::new(tree, bar, "Score 0", DEFAULT_FONT, Style::default())?;
    let lives = Label::new(tree, bar, "Lives 0", DEFAULT_FONT, Style::default())?;
    let level = Label::new(tree, bar, "Level 0", DEFAULT_FONT, Style::default())?;
    let banner = Label::new(tree, root, "", DEFAULT_FONT, hidden_style())?;
    Ok(HudNodes {
        score,
        lives,
        level,
        banner,
    })
}

fn banner_text(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Title => Some("FRACTURE - press Space to launch"),
        Phase::LevelComplete => Some("Level Complete"),
        Phase::GameOver => Some("Game Over"),
        Phase::Won => Some("You Win!"),
        Phase::Playing => None,
    }
}

fn update_banner(tree: &mut UiTree, banner: NodeId, phase: Phase) -> EngineResult<()> {
    match banner_text(phase) {
        Some(text) => {
            tree.set_text(banner, Some(text.to_string()))?;
            tree.set_style(banner, banner_style())?;
        }
        None => tree.set_style(banner, hidden_style())?,
    }
    Ok(())
}

fn update_hud(world: &World, tree: &mut UiTree, hud: &mut Hud) -> EngineResult<()> {
    let Some(nodes) = hud.nodes else {
        return Ok(());
    };
    let Some(state) = world.get_resource::<GameState>().map(|state| *state) else {
        return Ok(());
    };
    if hud.last == Some(state) {
        return Ok(());
    }
    let phase_changed = hud.last.map(|prev| prev.phase) != Some(state.phase);
    tree.set_text(nodes.score, Some(format!("Score {}", state.score)))?;
    tree.set_text(nodes.lives, Some(format!("Lives {}", state.lives)))?;
    tree.set_text(nodes.level, Some(format!("Level {}", state.level + 1)))?;
    if phase_changed {
        update_banner(tree, nodes.banner, state.phase)?;
    }
    hud.last = Some(state);
    Ok(())
}

pub fn install(app: &mut App) {
    let hud = Rc::new(RefCell::new(Hud {
        nodes: None,
        last: None,
    }));
    let setup_hud = Rc::clone(&hud);
    app.add_ui_setup(move |tree| {
        setup_hud.borrow_mut().nodes = Some(build_hud(tree)?);
        Ok(())
    });
    app.add_ui_update(move |world, tree| {
        let mut hud = hud.borrow_mut();
        update_hud(world, tree, &mut hud)
    });
}
