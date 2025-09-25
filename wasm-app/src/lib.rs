use std::cell::RefCell;
use std::rc::{Rc, Weak};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use esgea::{Action, Game, GameError, GameResult, NodeIndex, PlayerId};
use futures_util::StreamExt;
use iroh::endpoint::Connection;
use iroh::{Endpoint, NodeAddr, NodeId, Watcher};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    console, window, Event, HtmlButtonElement, HtmlDivElement, HtmlInputElement, HtmlOptionElement,
    HtmlSelectElement, HtmlUListElement,
};

const ALPN: &[u8] = b"esgea.demo.v1";

struct DemoApp {
    game: Game,
    active_player: PlayerId,
    log: Vec<String>,
    network: Option<Rc<RefCell<NetworkState>>>,
    default_location: NodeIndex,
}

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    active_player: usize,
    default_location: usize,
    locations: Vec<LocationSnapshot>,
    players: Vec<PlayerSnapshot>,
    log: Vec<String>,
    network_code: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
enum WireMessage {
    Snapshot(Snapshot),
    Action { player: usize, action: Action },
    EndTurn,
    Reset,
    RequestSnapshot,
}

struct NetworkState {
    endpoint: Endpoint,
    node_addr: Option<NodeAddr>,
    peers: Vec<Rc<PeerConnection>>,
    app: Weak<RefCell<DemoApp>>,
}

struct PeerConnection {
    connection: Rc<Connection>,
    remote: Option<NodeId>,
    state: Weak<RefCell<NetworkState>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<DemoApp>>>> = RefCell::new(None);
}

fn setup_demo_map(game: &mut Game) -> NodeIndex {
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

    alpha
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

fn encode_node_addr(addr: &NodeAddr) -> Result<String, JsValue> {
    serde_json::to_vec(addr)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|err| JsValue::from_str(&format!("Failed to serialise peer code: {err}")))
}

fn decode_node_addr(encoded: &str) -> Result<NodeAddr, JsValue> {
    let data = URL_SAFE_NO_PAD
        .decode(encoded.trim().as_bytes())
        .map_err(|err| JsValue::from_str(&format!("Invalid peer code: {err}")))?;
    serde_json::from_slice(&data)
        .map_err(|err| JsValue::from_str(&format!("Invalid peer code payload: {err}")))
}

impl DemoApp {
    fn new() -> Rc<RefCell<Self>> {
        let mut game = Game::new();
        let default_location = setup_demo_map(&mut game);
        let app = Rc::new(RefCell::new(DemoApp {
            game,
            active_player: 0,
            log: Vec::new(),
            network: None,
            default_location,
        }));
        {
            let mut this = app.borrow_mut();
            this.begin_turn();
        }
        DemoApp::initialise_network(app.clone());
        app
    }

    fn initialise_network(app: Rc<RefCell<Self>>) {
        let weak = Rc::downgrade(&app);
        spawn_local(async move {
            match NetworkState::create(weak.clone()).await {
                Ok(network) => {
                    if let Some(app_rc) = weak.upgrade() {
                        app_rc.borrow_mut().network = Some(network);
                        if let Err(err) = refresh_ui() {
                            console::error_1(&err);
                        }
                    }
                }
                Err(err) => console::error_1(&err),
            }
        });
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

    fn advance_turn(&mut self) {
        if self.game.players.is_empty() {
            return;
        }
        self.active_player = (self.active_player + 1) % self.game.players.len();
        self.begin_turn();
    }

    fn next_player(&mut self) {
        self.advance_turn();
        self.broadcast_message(WireMessage::EndTurn);
        self.broadcast_snapshot();
    }

    fn apply_action(&mut self, action: Action) -> Result<(), JsValue> {
        let action_clone = action.clone();
        map_result(self.game.do_action(self.active_player, action_clone.clone()))?;
        self.record_events();
        self.game.reset_event();
        self.broadcast_message(WireMessage::Action {
            player: self.active_player,
            action: action_clone,
        });
        self.broadcast_snapshot();
        Ok(())
    }

    fn broadcast_message(&self, message: WireMessage) {
        if let Some(network) = &self.network {
            NetworkState::broadcast(network, message);
        }
    }

    fn broadcast_snapshot(&self) {
        let snapshot = self.snapshot();
        self.broadcast_message(WireMessage::Snapshot(snapshot));
    }

    fn reset_state(&mut self, broadcast: bool) {
        self.game = Game::new();
        self.log.clear();
        self.active_player = 0;
        self.default_location = setup_demo_map(&mut self.game);
        self.begin_turn();
        if broadcast {
            self.broadcast_message(WireMessage::Reset);
            self.broadcast_snapshot();
        }
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

        let network_code = self
            .network
            .as_ref()
            .and_then(|network| network.borrow().share_code().ok());

        Snapshot {
            active_player: self.active_player,
            default_location: self.default_location.index(),
            locations,
            players,
            log: self.log.clone(),
            network_code,
        }
    }

    fn load_snapshot(&mut self, snapshot: Snapshot) {
        let mut new_game = Game::new();
        if snapshot.locations.is_empty() {
            return;
        }
        let max_id = snapshot
            .locations
            .iter()
            .map(|loc| loc.id)
            .max()
            .unwrap_or(0);
        let mut mapping = vec![NodeIndex::new(0); max_id + 1];
        let mut sorted_locations = snapshot.locations.clone();
        sorted_locations.sort_by_key(|loc| loc.id);
        for location in &sorted_locations {
            let index = new_game.add_location(location.name.clone(), location.base_income);
            mapping[location.id] = index;
            if let Some(node) = new_game.cities.node_weight_mut(index) {
                node.boost = location.boost;
                node.pending_powerup = location.pending_powerup;
                node.control = location.control;
            }
        }
        for location in &snapshot.locations {
            let from = mapping[location.id];
            for neighbor in &location.neighbors {
                if *neighbor < mapping.len() {
                    let to = mapping[*neighbor];
                    new_game.connect_locations(from, to);
                }
            }
        }
        new_game.players.clear();
        for player in snapshot.players {
            let location = if player.location < mapping.len() {
                mapping[player.location]
            } else {
                NodeIndex::new(0)
            };
            let id = new_game.spawn_player(location);
            let pl = &mut new_game.players[id];
            pl.alive = player.alive;
            pl.intel = player.intel;
            pl.hidden_signals = player.hidden_signals;
            pl.visible_violence = player.visible_violence;
            pl.active_scan = player.active_scan;
            pl.concealed = player.concealed;
            pl.invisible = player.invisible;
        }
        self.game = new_game;
        self.active_player = snapshot.active_player.min(self.game.players.len().saturating_sub(1));
        self.log = snapshot.log;
        self.default_location = NodeIndex::new(snapshot.default_location);
        self.game.reset_event();
    }

    fn handle_wire_message(&mut self, message: WireMessage) {
        match message {
            WireMessage::Snapshot(snapshot) => {
                self.load_snapshot(snapshot);
            }
            WireMessage::Action { player, action } => {
                if let Err(err) = self.game.do_action(player, action.clone()) {
                    console::warn_1(&JsValue::from_str(&format!(
                        "Ignoring remote action: {err:?}"
                    )));
                } else {
                    self.record_events();
                    self.game.reset_event();
                    self.active_player = player;
                }
            }
            WireMessage::EndTurn => {
                self.advance_turn();
            }
            WireMessage::Reset => {
                self.reset_state(false);
            }
            WireMessage::RequestSnapshot => {
                self.broadcast_snapshot();
            }
        }
    }

    fn connect_peer(&mut self, encoded: String) -> Result<(), JsValue> {
        let network = self
            .network
            .as_ref()
            .cloned()
            .ok_or_else(|| JsValue::from_str("Networking not ready"))?;
        let addr = decode_node_addr(&encoded)?;
        NetworkState::connect(&network, addr);
        Ok(())
    }
}

impl NetworkState {
    async fn create(app: Weak<RefCell<DemoApp>>) -> Result<Rc<RefCell<Self>>, JsValue> {
        let endpoint = Endpoint::builder()
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|err| JsValue::from_str(&format!("Failed to start iroh endpoint: {err:?}")))?;
        let mut watcher = endpoint.node_addr();
        let node_addr = watcher.initialized().await;
        let state = Rc::new(RefCell::new(NetworkState {
            endpoint: endpoint.clone(),
            node_addr: Some(node_addr),
            peers: Vec::new(),
            app,
        }));
        NetworkState::spawn_accept_loop(state.clone());
        NetworkState::spawn_node_addr_watcher(state.clone());
        Ok(state)
    }

    fn spawn_node_addr_watcher(state: Rc<RefCell<Self>>) {
        let endpoint = state.borrow().endpoint.clone();
        let weak_state = Rc::downgrade(&state);
        spawn_local(async move {
            let mut stream = endpoint.node_addr().stream();
            while let Some(maybe_addr) = stream.next().await {
                if let Some(addr) = maybe_addr {
                    if let Some(state_rc) = weak_state.upgrade() {
                        {
                            let mut state = state_rc.borrow_mut();
                            state.node_addr = Some(addr.clone());
                        }
                        if let Err(err) = refresh_ui() {
                            console::error_1(&err);
                        }
                    }
                }
            }
        });
    }

    fn spawn_accept_loop(state: Rc<RefCell<Self>>) {
        let endpoint = state.borrow().endpoint.clone();
        let weak_state = Rc::downgrade(&state);
        spawn_local(async move {
            while let Some(incoming) = endpoint.accept().await {
                match incoming.accept() {
                    Ok(connecting) => match connecting.await {
                        Ok(connection) => {
                            if let Some(state_rc) = weak_state.upgrade() {
                                NetworkState::register_connection(state_rc, connection);
                            }
                        }
                        Err(err) => console::error_1(&JsValue::from_str(&format!(
                            "Incoming connection failed: {err:?}"
                        ))),
                    },
                    Err(err) => console::error_1(&JsValue::from_str(&format!(
                        "Failed to accept connection: {err:?}"
                    ))),
                }
            }
        });
    }

    fn register_connection(state: Rc<RefCell<Self>>, connection: Connection) {
        let peer = PeerConnection::new(connection, &state);
        {
            let mut state_mut = state.borrow_mut();
            state_mut.peers.push(peer.clone());
        }
        let app = state.borrow().app.clone();
        if let Some(app_rc) = app.upgrade() {
            let snapshot = app_rc.borrow().snapshot();
            peer.send_async(WireMessage::Snapshot(snapshot));
        }
        peer.send_async(WireMessage::RequestSnapshot);
        if let Some(remote) = peer.remote {
            console::log_1(&JsValue::from_str(&format!(
                "Connected to peer {}",
                remote
            )));
        }
    }

    fn broadcast(network: &Rc<RefCell<Self>>, message: WireMessage) {
        let peers = network.borrow().peers.clone();
        for peer in peers {
            peer.send_async(message.clone());
        }
    }

    fn connect(network: &Rc<RefCell<Self>>, addr: NodeAddr) {
        let endpoint = network.borrow().endpoint.clone();
        let weak_state = Rc::downgrade(network);
        spawn_local(async move {
            match endpoint.connect(addr.clone(), ALPN).await {
                Ok(connection) => {
                    if let Some(state_rc) = weak_state.upgrade() {
                        NetworkState::register_connection(state_rc, connection);
                    }
                }
                Err(err) => console::error_1(&JsValue::from_str(&format!(
                    "Failed to connect to peer: {err:?}"
                ))),
            }
        });
    }

    fn handle_message(&self, message: WireMessage) {
        if let Some(app_rc) = self.app.upgrade() {
            {
                let mut app = app_rc.borrow_mut();
                app.handle_wire_message(message);
            }
            if let Err(err) = refresh_ui() {
                console::error_1(&err);
            }
        }
    }

    fn share_code(&self) -> Result<String, JsValue> {
        self
            .node_addr
            .as_ref()
            .map(encode_node_addr)
            .unwrap_or_else(|| Ok(String::from("initialising…")))
    }
}

impl PeerConnection {
    fn new(connection: Connection, state: &Rc<RefCell<NetworkState>>) -> Rc<Self> {
        let connection = Rc::new(connection);
        let remote = connection.remote_node_id().ok();
        let peer = Rc::new(PeerConnection {
            connection: connection.clone(),
            remote,
            state: Rc::downgrade(state),
        });
        PeerConnection::start_reader(peer.clone());
        peer
    }

    fn start_reader(peer: Rc<Self>) {
        let connection = peer.connection.clone();
        let weak_state = peer.state.clone();
        spawn_local(async move {
            loop {
                match connection.accept_uni().await {
                    Ok(mut recv) => {
                        match recv.read_to_end(64 * 1024).await {
                            Ok(data) => match serde_json::from_slice::<WireMessage>(&data) {
                                Ok(message) => {
                                    if let Some(state_rc) = weak_state.upgrade() {
                                        state_rc.borrow().handle_message(message);
                                    }
                                }
                                Err(err) => console::error_1(&JsValue::from_str(&format!(
                                    "Failed to decode message: {err}"
                                ))),
                            },
                            Err(err) => console::error_1(&JsValue::from_str(&format!(
                                "Failed to read stream: {err}"
                            ))),
                        }
                    }
                    Err(err) => {
                        console::error_1(&JsValue::from_str(&format!(
                            "Connection closed: {err:?}"
                        )));
                        break;
                    }
                }
            }
            if let Some(state_rc) = weak_state.upgrade() {
                state_rc
                    .borrow_mut()
                    .peers
                    .retain(|existing| !Rc::ptr_eq(existing, &peer));
            }
        });
    }

    fn send_async(&self, message: WireMessage) {
        let connection = self.connection.clone();
        spawn_local(async move {
            if let Err(err) = send_message(connection, message).await {
                console::error_1(&JsValue::from_str(&format!(
                    "Failed to send message: {err}"
                )));
            }
        });
    }
}

async fn send_message(connection: Rc<Connection>, message: WireMessage) -> Result<(), String> {
    let data = serde_json::to_vec(&message).map_err(|err| err.to_string())?;
    let mut stream = connection.open_uni().await.map_err(|err| err.to_string())?;
    stream.write_all(&data).await.map_err(|err| err.to_string())?;
    stream.finish().map_err(|err| err.to_string())?;
    Ok(())
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

fn render_locations(locations: &[LocationSnapshot], default_location: usize) -> Result<(), JsValue> {
    let doc = document()?;
    let container: HtmlDivElement = doc
        .get_element_by_id("gameboard")
        .ok_or_else(|| JsValue::from_str("missing gameboard"))?
        .dyn_into()?;
    container.set_inner_html("");
    for location in locations {
        let element = doc.create_element("div")?;
        let classes = if location.id == default_location {
            "location default-location"
        } else {
            "location"
        };
        element.set_class_name(classes);
        let owner = location
            .control
            .map(|ctrl| format!("Controlled by Player {}", ctrl))
            .unwrap_or_else(|| "Uncontrolled".to_string());
        let players = if location.players.is_empty() {
            "None".to_string()
        } else {
            location
                .players
                .iter()
                .map(|p| format!("P{}", p))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let badge = if location.id == default_location {
            "<p class=\"badge\">Default drop point</p>"
        } else {
            ""
        };
        element.set_inner_html(&format!(
            "<h3>{}</h3><p>Income: {}</p><p>{}</p><p>Players here: {}</p>{}",
            location.name, location.base_income, owner, players, badge
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

fn render_network(code: Option<&str>) -> Result<(), JsValue> {
    let display = code.unwrap_or("initialising…");
    set_text("peer_code", display)?;
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
            render_locations(&snapshot.locations, snapshot.default_location)?;
            render_players(&snapshot.players)?;
            render_log(&snapshot.log)?;
            render_network(snapshot.network_code.as_deref())?;
            update_move_targets(&app)?;
            update_reveal_targets(&app)?;
            Ok(())
        } else {
            Err(JsValue::from_str("Application not initialised"))
        }
    })
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
        let payload = if target < 0 {
            Action::Reveal(None)
        } else {
            Action::Reveal(Some(target as usize))
        };
        app.apply_action(payload)
    })
}

#[wasm_bindgen]
pub fn connect_to_peer(code: String) -> Result<(), JsValue> {
    with_app(|app| app.connect_peer(code))
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
    with_app(|app| {
        app.reset_state(true);
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

    event_button("connect_peer", move |_: Event| {
        let doc = document()?;
        let input: HtmlInputElement = doc
            .get_element_by_id("peer_input")
            .ok_or_else(|| JsValue::from_str("missing peer input"))?
            .dyn_into()?;
        let value = input.value();
        if value.trim().is_empty() {
            return Ok(());
        }
        connect_to_peer(value)?;
        input.set_value("");
        Ok(())
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
