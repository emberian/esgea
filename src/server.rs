use actix::prelude::*;
use actix_session::{storage::CookieSessionStore, SessionMiddleware};
use actix_web::cookie::Key;
use actix_web::web::{Bytes, Data};
use actix_web::{
    get, http::header::ContentType, middleware::Logger, web, App, HttpResponse, HttpServer,
    Responder,
};
use actix_web::{http::header, post};
use actix_web::{Error, HttpRequest};
use actix_web_actors::ws;
use parking_lot::Mutex;
use petgraph::graph::NodeIndex;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{Receiver, Sender};

use tokio::process::Command;

struct GameState {
    game: Arc<Mutex<esgea::Game>>,
    pid_channels: Vec<Option<Addr<ReceiverStream>>>,
}

impl GameState {
    fn new() -> Self {
        Self {
            game: Arc::new(Mutex::new(esgea::Game::new())),
            pid_channels: vec![],
        }
    }
}

struct State {
    games: BTreeMap<u128, GameState>,
}

#[get("/")]
async fn index() -> impl Responder {
    let index_html = std::fs::read("./src/index.html").expect("index?");

    HttpResponse::Ok()
        .append_header(ContentType::html())
        .body(index_html)
}

#[post("/start_game")]
async fn start_game(state: Data<Mutex<State>>) -> impl Responder {
    let mut st = state.lock();
    let gid: u128 = rand::random();
    st.games.insert(gid, GameState::new());
    HttpResponse::Ok()
        .append_header(ContentType::plaintext())
        .body(format!("{}", gid))
}

#[get("/lobby")]
async fn list_games(state: Data<Mutex<State>>) -> impl Responder {
    HttpResponse::Ok().append_header(ContentType::json()).json(
        state
            .lock()
            .games
            .iter()
            .map(|(gid, gm)| ((gm.game.lock().clone(), gid.to_string())))
            .collect::<Vec<_>>(),
    )
}

struct ReceiverStream;

impl core::ops::Drop for ReceiverStream {
    fn drop(&mut self) {
        println!("dropping a channel");
    }
}

impl Actor for ReceiverStream {
    type Context = ws::WebsocketContext<Self>;
}

/// Handler for `ws::Message`
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for ReceiverStream {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            _ => {}
        }
    }
}

struct Upd(usize, esgea::Observation);
impl Message for Upd {
    type Result = ();
}

impl Handler<Upd> for ReceiverStream {
    type Result = ();
    fn handle(&mut self, msg: Upd, ctx: &mut Self::Context) {
        ctx.text(serde_json::to_string(&(msg.0, msg.1)).expect("jsonify reactor supercritical"))
    }
}

#[get("/events/{gid}/{pid}")]
async fn event_stream(
    state: Data<Mutex<State>>,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    stream: web::Payload,
) -> Result<HttpResponse, Error> {
    let (gid, pid) = path.into_inner();
    let gid: u128 = gid.parse().expect("sad gid");
    let pid: esgea::PlayerId = pid.parse().expect("sad pid");
    println!("getting event stream for {gid}/{pid}");
    let actor = ReceiverStream;
    let mut res = ws::handshake(&req)?;

    let (addr, stream) = ws::WebsocketContext::create_with_addr(actor, stream);
    state.lock().games.entry(gid).and_modify(|e| {
        if pid < e.pid_channels.len() {
            e.pid_channels[pid] = Some(addr)
        }
    });

    Ok(res.streaming(stream))
}

#[post("/join_game/{gid}")]
async fn join_game(state: Data<Mutex<State>>, path: web::Path<String>) -> impl Responder {
    let mut st = state.lock();
    let gid = path.into_inner();
    println!("gid = {}", gid);
    let gid: u128 = gid.parse().expect("sad gid");
    match st.games.get_mut(&gid) {
        Some(gm) => {
            gm.pid_channels.push(None);
            let mut gm = gm.game.lock();
            let new_player = gm
                .players
                .last()
                .cloned()
                .map(|last| esgea::Player {
                    id: last.id + 1,
                    ..last
                })
                .unwrap_or(Default::default());
            println!("adding player to game {gid}: {new_player:?}");
            gm.players.push(new_player);
            gm.updates.insert(new_player.id, vec![]);
            HttpResponse::Ok()
                .append_header(ContentType::plaintext())
                .body(format!("{}", new_player.id))
        }
        None => HttpResponse::NotFound().body("no game"),
    }
}

#[get("/render/{gid}/{pid}")]
async fn render(state: Data<Mutex<State>>, path: web::Path<(String, String)>) -> impl Responder {
    let st = state.lock();
    let (gid, pid) = path.into_inner();
    let gid: u128 = gid.parse().expect("gid isnt u128");
    let pid: esgea::PlayerId = pid.parse().expect("pid isnt usize");

    let graphviz_source = st
        .games
        .get(&gid)
        .expect("no game?")
        .game
        .lock()
        .render(pid);
    let mut child = Command::new("dot")
        .arg("-Tsvg")
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .expect("graphviz failed");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(graphviz_source.as_bytes())
        .await
        .expect("writing");
    drop(stdin);
    let mut stdout = child.stdout.take().unwrap();
    let mut svg = vec![];
    stdout.read_to_end(&mut svg).await.expect("reading");
    HttpResponse::Ok()
        .append_header(ContentType::plaintext())
        .body(svg)
}

async fn distribute_updates(
    gs: &mut GameState,
    updates: Vec<(Option<esgea::PlayerId>, esgea::Observation)>,
) {
    let mut game = gs.game.lock();
    for (pid, upd) in updates {
        if let Some(pid) = pid {
            let seqno = game.updates[pid].len();
            game.updates[pid].push(upd.clone());
            if let Some(tx) = &gs.pid_channels[pid] {
                let result = tx.send(Upd(seqno, upd)).await;
                if let Err(eeeeee) = result {
                    println!("{} sending to {}, dropping delivery", eeeeee, pid);
                    gs.pid_channels[pid] = None;
                }
            } else {
                println!("no active event stream for {pid} -- cannot send {upd:?}");
            }
        } else {
            for pl in 0..game.updates.len() {
                let seqno = game.updates[pl].len();
                game.updates[pl].push(upd.clone());
                if let Some(tx) = &gs.pid_channels[pl] {
                    let result = tx.send(Upd(seqno, upd.clone())).await;
                    if let Err(_) = result {
                        gs.pid_channels[pl] = None;
                    }
                } else {
                    println!("no active event stream for {pl} -- cannot send {upd:?}");
                }
            }
        }
    }
}

#[post("/do_action/{gid}/{pid}")]
async fn do_action(
    state: Data<Mutex<State>>,
    path: web::Path<(String, String)>,
    body: Bytes,
) -> impl Responder {
    let (gid, pid) = path.into_inner();
    let gid: u128 = gid.parse().expect("gid isnt u128");
    let pid: esgea::PlayerId = pid.parse().expect("pid isnt usize");

    let mut guard = state.lock();
    let mut gs = guard.games.get_mut(&gid).expect("no homie");
    match body.as_ref() {
        b"strike" => {
            let upds = gs.game.lock().strike(pid);
            distribute_updates(&mut gs, upds).await
        }
        b"wait" => {
            let upds = gs.game.lock().wait(pid);
            distribute_updates(&mut gs, upds).await
        }
        b"capture" => {
            let upds = gs.game.lock().capture(pid);
            distribute_updates(&mut gs, upds).await
        }
        b"hide_signals" => {
            let upds = gs.game.lock().hide_signals(pid);
            distribute_updates(&mut gs, upds).await
        }
        b"invisible" => {
            let upds = gs.game.lock().invisible(pid);
            distribute_updates(&mut gs, upds).await
        }
        b"prepare" => {
            let upds = gs.game.lock().prepare(pid);
            distribute_updates(&mut gs, upds).await
        }
        _ => match body.as_ref().split(|c| b':' == *c).collect::<Vec<_>>()[..] {
            [b"move", to] => {
                return HttpResponse::Ok().body(
                    // TODO: fix try_move to give events
                    gs.game
                        .lock()
                        .try_move(
                            pid,
                            NodeIndex::new(
                                std::str::from_utf8(to)
                                    .expect("utf8")
                                    .parse()
                                    .expect("bad location"),
                            ),
                        )
                        .to_string(),
                );
            }
            [b"reveal", who] => {
                gs.game.lock().reveal(
                    pid, // TODO
                    None,
                );
            }
            _ => return HttpResponse::InternalServerError().body("no such action"),
        },
    }
    HttpResponse::Ok().body(())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let secret_key = Key::generate();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

    let data = Data::new(Mutex::new(State {
        games: BTreeMap::new(),
    }));

    HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .wrap(SessionMiddleware::new(
                CookieSessionStore::default(),
                secret_key.clone(),
            ))
            .wrap(Logger::new("%U"))
            .service(index)
            .service(do_action)
            .service(list_games)
            .service(join_game)
            .service(event_stream)
            .service(render)
            .service(start_game)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
