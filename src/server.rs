use actix_web::{get, http::header::ContentType, web, App, HttpResponse, HttpServer, Responder};

#[get("/")]
async fn index() -> impl Responder {
    let index_html = include_str!("index.html");

    HttpResponse::Ok()
        .append_header(ContentType::html())
        .body(index_html)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| App::new().service(index))
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
