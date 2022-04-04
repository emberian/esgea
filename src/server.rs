use actix_session::{storage::CookieSessionStore, Session, SessionMiddleware};
use actix_web::cookie::Key;
use actix_web::web::{Bytes, Data};
use actix_web::{
    get, http::header::ContentType, middleware::Logger, web, App, HttpResponse, HttpServer,
    Responder,
};
use actix_web::{post, HttpMessage, HttpRequest};
use parking_lot::Mutex;
use petgraph::graph::NodeIndex;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use tokio::process::Command;

struct GameState {
  game: Arc<Mutex<esgea::Game>>
}

impl GameState {
  fn new() -> Self {
    Self {
      game: Arc::new(Mutex::new(esgea::Game::new()))
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
    st.games
        .insert(gid, GameState::new());
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

#[post("/join_game/{gid}")]
async fn join_game(state: Data<Mutex<State>>, path: web::Path<String>) -> impl Responder {
    let st = state.lock();
    let gid = path.into_inner();
    println!("gid = {}", gid);
    let gid: u128 = gid.parse().expect("sad gid");
    match st.games.get(&gid) {
        Some(gm) => {
            let mut gm = gm.game.lock();
            let last = gm.players.last().cloned().unwrap_or(Default::default());
            gm.players.push(esgea::Player {
                id: last.id + 1,
                ..last
            });
            gm.updates.push(vec![]);
            HttpResponse::Ok()
                .append_header(ContentType::plaintext())
                .body(format!("{}", last.id + 1))
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

    let graphviz_source = st.games.get(&gid).expect("no game?").game.lock().render(pid);
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

fn distribute_updates(game: &mut esgea::Game, updates: Vec<(Option<esgea::PlayerId>, esgea::ClientUpdate)>) {
  for (pid, upd) in updates {
    if let Some(pid) = pid {
      game.updates[pid].push(upd);
    } else {
      for pl in 0..game.updates.len() {
        game.updates[pl].push(upd.clone());
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

    let gm = state.lock().games.get(&gid).expect("no homie").game.clone();
    let mut gm = gm.lock();
    match body.as_ref() {
        b"strike" => { let upds = gm.strike(pid); distribute_updates(&mut gm, upds) },
        b"wait" => { gm.wait(pid); },
        b"capture" => { gm.capture(pid); },
        b"hide_signals" => { gm.hide_signals(pid); },
        b"invisible" => { gm.invisible(pid); },
        b"prepare" => { gm.prepare(pid); },
        _ => match body.as_ref().split(|c| b':' == *c).collect::<Vec<_>>()[..] {
            [b"move", to] => {
                return HttpResponse::Ok().body(
                    gm.try_move(
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
                gm.reveal(
                    pid,
                    // TODO
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
            .service(render)
            .service(start_game)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
