use std::cell::RefCell;
use std::rc::Rc;

use esgea::NodeIndex;
use esgea::{Action, Game, GameError, GameResult, PlayerId};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{console, window, Event, HtmlButtonElement, HtmlDivElement, HtmlOptionElement, HtmlSelectElement, HtmlUListElement};

struct DemoApp {
    game: Game,
    active_player: PlayerId,
    log: Vec<String>,
}

#[derive(Serialize)]
struct LocationSnapshot {
    id: usize,
    name: String,
    base_income: u32,
    control: Option<usize>,
    boost: bool,
    pending_powerup: Option<u32>,
    players: Vec<usize>,
    neighbors: Vec<usize>,
}

#[derive(Serialize)]
struct PlayerSnapshot {
    id: usize,
    alive: bool,
    intel: u32,
    hidden_signals: bool,
    visible_violence: bool,
    active_scan: bool,
    concealed: bool,
    invisible: bool,
    location: usize,
}

#[derive(Serialize)]
struct Snapshot {
    active_player: usize,
    locations: Vec<LocationSnapshot>,
    players: Vec<PlayerSnapshot>,
    log: Vec<String>,
}

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<DemoApp>>>> = RefCell::new(None);
}

fn setup_demo_map(game: &mut Game) {
    let alpha = game.add_location("Alpha", 2);
    let bravo = game.add_location("Bravo", 1);
    let charlie = game.add_location("Charlie", 1);
    let delta = game.add_location("Delta", 3);
    game.connect_locations(alpha, bravo);
    game.connect_locations(bravo, charlie);
    game.connect_locations(charlie, delta);
    game.connect_locations(alpha, delta);

    let p0 = game.spawn_player(alpha);
    let p1 = game.spawn_player(delta);

    if let Some(loc) = game.cities.node_weight_mut(alpha) {
        loc.control = Some(p0);
    }
    if let Some(loc) = game.cities.node_weight_mut(delta) {
        loc.control = Some(p1);
    }

    game.players[p0].intel = 4;
    game.players[p1].intel = 4;
}

fn map_result(result: GameResult) -> Result<(), JsValue> {
    match result {
        Ok(()) => Ok(()),
        Err(err) => Err(JsValue::from_str(match err {
            GameError::NotEnoughIntel => "Not enough intel",
            GameError::NotYourTurn => "Not your turn",
            GameError::WouldNoop => "Action would have no effect",
        })),
    }
}

impl DemoApp {
    fn new() -> Rc<RefCell<Self>> {
        let mut game = Game::new();
        setup_demo_map(&mut game);
        let mut app = DemoApp {
            game,
            active_player: 0,
            log: Vec::new(),
        };
        app.begin_turn();
        Rc::new(RefCell::new(app))
    }

    fn begin_turn(&mut self) {
        self.game.start_turn(self.active_player);
        self.record_events();
        self.game.reset_event();
    }

    fn record_events(&mut self) {
        for obs in &self.game.event.public_observations {
            self.log.push(obs.describe());
        }
        for (pid, observations) in &self.game.event.private_observations {
            for obs in observations {
                self.log
                    .push(format!("[P{}] {}", pid, obs.describe()));
            }
        }
    }

    fn next_player(&mut self) {
        if self.game.players.is_empty() {
            return;
        }
        self.active_player = (self.active_player + 1) % self.game.players.len();
        self.begin_turn();
    }

    fn apply_action(&mut self, action: Action) -> Result<(), JsValue> {
        map_result(self.game.do_action(self.active_player, action))?;
        self.record_events();
        self.game.reset_event();
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        let mut locations = Vec::new();
        for location in self.game.locations() {
            let mut players = Vec::new();
            for player in &self.game.players {
                if player.location == location.index {
                    players.push(player.id);
                }
            }
            locations.push(LocationSnapshot {
                id: location.index.index(),
                name: location.name,
                base_income: location.base_income,
                control: location.control,
                boost: location.boost,
                pending_powerup: location.pending_powerup,
                players,
                neighbors: self
                    .game
                    .neighbors(location.index)
                    .into_iter()
                    .map(|idx| idx.index())
                    .collect(),
            });
        }

        let players = self
            .game
            .players
            .iter()
            .map(|player| PlayerSnapshot {
                id: player.id,
                alive: player.alive,
                intel: player.intel,
                hidden_signals: player.hidden_signals,
                visible_violence: player.visible_violence,
                active_scan: player.active_scan,
                concealed: player.concealed,
                invisible: player.invisible,
                location: player.location.index(),
            })
            .collect();

        Snapshot {
            active_player: self.active_player,
            locations,
            players,
            log: self.log.clone(),
        }
    }
}

fn with_app<F>(f: F) -> Result<(), JsValue>
where
    F: FnOnce(&mut DemoApp) -> Result<(), JsValue>,
{
    APP.with(|app| {
        if let Some(app) = &*app.borrow() {
            let mut app = app.borrow_mut();
            f(&mut app)
        } else {
            Err(JsValue::from_str("Application not initialised"))
        }
    })
}

fn snapshot_value() -> Result<JsValue, JsValue> {
    APP.with(|app| {
        if let Some(app) = &*app.borrow() {
            let app = app.borrow();
            serde_wasm_bindgen::to_value(&app.snapshot())
                .map_err(|err| JsValue::from_str(&err.to_string()))
        } else {
            Err(JsValue::from_str("Application not initialised"))
        }
    })
}

fn document() -> Result<web_sys::Document, JsValue> {
    window()
        .ok_or_else(|| JsValue::from_str("missing window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("missing document"))
}

fn set_text(id: &str, text: &str) -> Result<(), JsValue> {
    let doc = document()?;
    if let Some(element) = doc.get_element_by_id(id) {
        element.set_text_content(Some(text));
    }
    Ok(())
}

fn render_locations(locations: &[LocationSnapshot]) -> Result<(), JsValue> {
    let doc = document()?;
    let container: HtmlDivElement = doc
        .get_element_by_id("gameboard")
        .ok_or_else(|| JsValue::from_str("missing gameboard"))?
        .dyn_into()?;
    container.set_inner_html("");
    for location in locations {
        let element = doc.create_element("div")?;
        element.set_class_name("location");
        let owner = location
            .control
            .map(|ctrl| format!("Controlled by Player {}", ctrl))
            .unwrap_or_else(|| "Uncontrolled".to_string());
        element.set_inner_html(&format!(
            "<h3>{}</h3><p>Income: {}</p><p>{}</p><p>Players here: {}</p>",
            location.name,
            location.base_income,
            owner,
            if location.players.is_empty() {
                "None".to_string()
            } else {
                location
                    .players
                    .iter()
                    .map(|p| format!("P{}", p))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        container.append_child(&element)?;
    }
    Ok(())
}

fn render_players(players: &[PlayerSnapshot]) -> Result<(), JsValue> {
    let doc = document()?;
    let list: HtmlUListElement = doc
        .get_element_by_id("players")
        .ok_or_else(|| JsValue::from_str("missing players list"))?
        .dyn_into()?;
    list.set_inner_html("");
    for player in players {
        let item = doc.create_element("li")?;
        item.set_inner_html(&format!(
            "Player {} – Intel: {} – Location: {}{}",
            player.id,
            player.intel,
            player.location,
            if player.alive { "" } else { " (eliminated)" }
        ));
        list.append_child(&item)?;
    }
    Ok(())
}

fn render_log(log: &[String]) -> Result<(), JsValue> {
    let doc = document()?;
    let list: HtmlUListElement = doc
        .get_element_by_id("log")
        .ok_or_else(|| JsValue::from_str("missing log list"))?
        .dyn_into()?;
    list.set_inner_html("");
    for entry in log.iter().rev().take(15) {
        let item = doc.create_element("li")?;
        item.set_text_content(Some(entry));
        list.append_child(&item)?;
    }
    Ok(())
}

fn update_move_targets(app: &DemoApp) -> Result<(), JsValue> {
    let doc = document()?;
    let select: HtmlSelectElement = doc
        .get_element_by_id("move_target")
        .ok_or_else(|| JsValue::from_str("missing move target"))?
        .dyn_into()?;
    select.set_inner_html("");
    if let Some(player) = app.game.players.get(app.active_player) {
        for neighbor in app.game.neighbors(player.location) {
            let option: HtmlOptionElement = doc.create_element("option")?.dyn_into()?;
            option.set_value(&neighbor.index().to_string());
            option.set_text(&format!("{}", neighbor.index()));
            select.add_with_html_option_element(&option)?;
        }
    }
    Ok(())
}

fn update_reveal_targets(app: &DemoApp) -> Result<(), JsValue> {
    let doc = document()?;
    let select: HtmlSelectElement = doc
        .get_element_by_id("reveal_target")
        .ok_or_else(|| JsValue::from_str("missing reveal target"))?
        .dyn_into()?;
    select.set_inner_html("");
    let option: HtmlOptionElement = doc.create_element("option")?.dyn_into()?;
    option.set_value("-1");
    option.set_text("Scan current location");
    select.add_with_html_option_element(&option)?;
    for player in &app.game.players {
        if player.id != app.active_player {
            let option: HtmlOptionElement = doc.create_element("option")?.dyn_into()?;
            option.set_value(&player.id.to_string());
            option.set_text(&format!("Reveal player {}", player.id));
            select.add_with_html_option_element(&option)?;
        }
    }
    Ok(())
}

fn refresh_ui() -> Result<(), JsValue> {
    APP.with(|app| {
        if let Some(app_rc) = &*app.borrow() {
            let app = app_rc.borrow();
            let snapshot = app.snapshot();
            set_text("pid", &format!("Player {}", snapshot.active_player))?;
            render_locations(&snapshot.locations)?;
            render_players(&snapshot.players)?;
            render_log(&snapshot.log)?;
            update_move_targets(&app)?;
            update_reveal_targets(&app)?;
            Ok(())
        } else {
            Err(JsValue::from_str("Application not initialised"))
        }
    })
}

fn action_button(id: &str, handler: impl Fn() -> Result<(), JsValue> + 'static) -> Result<(), JsValue> {
    let doc = document()?;
    let button: HtmlButtonElement = doc
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str("missing button"))?
        .dyn_into()?;
    let closure = Closure::wrap(Box::new(move || {
        if let Err(err) = handler() {
            console::error_1(&err);
        }
        if let Err(err) = refresh_ui() {
            console::error_1(&err);
        }
    }) as Box<dyn FnMut()>);
    button.set_onclick(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    Ok(())
}

fn event_button(id: &str, handler: impl Fn(Event) -> Result<(), JsValue> + 'static) -> Result<(), JsValue> {
    let doc = document()?;
    let button: HtmlButtonElement = doc
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str("missing button"))?
        .dyn_into()?;
    let closure = Closure::wrap(Box::new(move |event: Event| {
        if let Err(err) = handler(event.clone()) {
            console::error_1(&err);
        }
        if let Err(err) = refresh_ui() {
            console::error_1(&err);
        }
    }) as Box<dyn FnMut(_)>);
    button.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())?;
    closure.forget();
    Ok(())
}

#[wasm_bindgen]
pub fn snapshot() -> Result<JsValue, JsValue> {
    snapshot_value()
}

#[wasm_bindgen]
pub fn strike() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::Strike))
}

#[wasm_bindgen]
pub fn wait_turn() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::Wait))
}

#[wasm_bindgen]
pub fn capture() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::Capture))
}

#[wasm_bindgen]
pub fn hide_signals() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::HideSignals))
}

#[wasm_bindgen]
pub fn go_invisible() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::Invisible))
}

#[wasm_bindgen]
pub fn prepare() -> Result<(), JsValue> {
    with_app(|app| app.apply_action(Action::Prepare))
}

#[wasm_bindgen]
pub fn move_to(target: usize) -> Result<(), JsValue> {
    with_app(|app| {
        let node = NodeIndex::new(target);
        app.apply_action(Action::Move(node))
    })
}

#[wasm_bindgen]
pub fn reveal(target: i32) -> Result<(), JsValue> {
    with_app(|app| {
        if target < 0 {
            map_result(app.game.reveal_action(app.active_player, None))?;
        } else {
            map_result(app.game.reveal_action(app.active_player, Some(target as usize)))?;
        }
        app.record_events();
        app.game.reset_event();
        Ok(())
    })
}

#[wasm_bindgen]
pub fn end_turn() -> Result<(), JsValue> {
    with_app(|app| {
        app.next_player();
        Ok(())
    })
}

#[wasm_bindgen]
pub fn reset_game() -> Result<(), JsValue> {
    APP.with(|app| {
        let new_app = DemoApp::new();
        *app.borrow_mut() = Some(new_app);
        Ok(())
    })
}

fn init_app() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    APP.with(|app| {
        if app.borrow().is_none() {
            *app.borrow_mut() = Some(DemoApp::new());
        }
    });

    action_button("strike", || strike().map(|_| ()))?;
    action_button("wait", || wait_turn().map(|_| ()))?;
    action_button("capture", || capture().map(|_| ()))?;
    action_button("hide_signals", || hide_signals().map(|_| ()))?;
    action_button("invisible", || go_invisible().map(|_| ()))?;
    action_button("prepare", || prepare().map(|_| ()))?;

    event_button("move", move |_: Event| {
        let doc = document()?;
        let select: HtmlSelectElement = doc
            .get_element_by_id("move_target")
            .ok_or_else(|| JsValue::from_str("missing move target"))?
            .dyn_into()?;
        let value = select.value();
        if value.is_empty() {
            return Ok(());
        }
        let target: usize = value.parse().map_err(|_| JsValue::from_str("invalid move"))?;
        move_to(target)
    })?;

    event_button("reveal_btn", move |_: Event| {
        let doc = document()?;
        let select: HtmlSelectElement = doc
            .get_element_by_id("reveal_target")
            .ok_or_else(|| JsValue::from_str("missing reveal target"))?
            .dyn_into()?;
        let value = select.value();
        let target = value.parse::<i32>().unwrap_or(-1);
        reveal(target)
    })?;

    action_button("end_turn", || end_turn().map(|_| ()))?;
    action_button("reset", || reset_game().map(|_| ()))?;

    refresh_ui()?;
    Ok(())
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    init_app()
}
