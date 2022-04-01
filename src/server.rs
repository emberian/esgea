use actix_session::{storage::CookieSessionStore, Session, SessionMiddleware};
use actix_web::cookie::Key;
use actix_web::web::{Bytes, Data};
use actix_web::{
    get, http::header::ContentType, middleware::Logger, web, App, HttpResponse, HttpServer,
    Responder,
};
use actix_web::{post, HttpMessage, HttpRequest};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;

struct State {
    games: BTreeMap<u128, Arc<Mutex<esgea::Game>>>,
}

#[get("/")]
async fn index() -> impl Responder {
    let index_html = include_str!("index.html");

    HttpResponse::Ok()
        .append_header(ContentType::html())
        .body(index_html)
}

#[post("/start_game")]
async fn start_game(state: Data<Mutex<State>>, sess: Session) -> impl Responder {
    let mut st = state.lock();
    let gid: u128 = rand::random();
    st.games
        .insert(gid, Arc::new(Mutex::new(esgea::Game::new())));
    sess.insert("gid", gid).expect("gid json sadness");
    sess.insert("pid", 0usize).expect("pid json sadness");
    HttpResponse::Ok()
        .append_header(ContentType::plaintext())
        .body(format!("{}", gid))
}

#[post("/do_action")]
async fn do_action(state: Data<Mutex<State>>, sess: Session, body: Bytes) -> impl Responder {
    println!("{:?}", sess.entries());
    let gid = sess.get::<u128>("gid").unwrap().expect("gid not set");
    let pid = sess.get::<usize>("pid").unwrap().expect("pid not set");

    let gm = state.lock().games.get(&gid).expect("no homie").clone();
    let mut gm = gm.lock();
    match body.as_ref() {
        b"strike" => gm.strike(pid),
        b"wait" => gm.wait(pid),
        b"capture" => gm.capture(pid),
        b"hide" => gm.hide(pid),
        b"reveal" => gm.reveal(pid),
        b"invisible" => gm.invisible(pid),
        b"prepare" => gm.prepare(pid),
        _ => return HttpResponse::BadRequest(),
    }
    HttpResponse::Ok()
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let secret_key = Key::generate();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

    HttpServer::new(move || {
        App::new()
            .app_data(Data::new(Mutex::new(State {
                games: BTreeMap::new(),
            })))
            .wrap(SessionMiddleware::new(
                CookieSessionStore::default(),
                secret_key.clone(),
            ))
            .wrap(Logger::new("%U"))
            .service(index)
            .service(do_action)
            .service(start_game)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
