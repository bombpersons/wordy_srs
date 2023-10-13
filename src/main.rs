use std::{net::SocketAddr, error::Error, sync::Arc, env};
use serde::{Deserialize, Serialize};

use askama::Template;
use axum::{
    routing::{get, post},
    Router, extract::{State, Query}, Form, Json,
};
use log::{info, error};
use tower_http::services::ServeDir;

mod knowledge;
use knowledge::Knowledge;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
}

async fn index_get(State(knowledge): State<Knowledge>) -> IndexTemplate {
    IndexTemplate { }
}

#[derive(Template)]
#[template(path = "add.html")]
struct AddTemplate {
}

async fn add_get(State(knowledge): State<Knowledge>) -> AddTemplate {
    AddTemplate { }
}

#[derive(Deserialize)]
struct AddTextQuery {
    text: String
}

async fn add_post(State(mut knowledge): State<Knowledge>,
                  Form(AddTextQuery{ text }): Form<AddTextQuery>) -> &'static str 
{
    knowledge.add_text(text.as_str()).await;

    "Sentences added."
}

#[derive(Template)]
#[template(path = "review.html")]
struct ReviewTemplate {
    sentence_id: i64,
    sentence: String,
    reviews_today_count: i64,
    words_being_reviewed: Vec<String>,
    words_that_are_new: Vec<String>
}

async fn review_get(State(knowledge): State<Knowledge>) -> ReviewTemplate {
    let review_info = knowledge.get_review_info().await;
    let sentence_data = knowledge.get_next_sentence_i_plus_one().await;

    ReviewTemplate {
        sentence_id: sentence_data.sentence_id,
        sentence: sentence_data.sentence_text,
        reviews_today_count: review_info.reviews_remaining,
        words_being_reviewed: sentence_data.words_being_reviewed.iter().map(|(_, text)| text.clone()).collect(),
        words_that_are_new: sentence_data.words_that_are_new.iter().map(|(_, text)| text.clone()).collect()
    }
}

#[derive(Deserialize)]
struct ReviewQuery {
    review_sentence_id: i64,
    response_quality: f64
}

#[derive(Serialize)]
struct ReviewResponse {
    success: bool
}

async fn review_post(State(knowledge): State<Knowledge>,
                     Json(ReviewQuery{ review_sentence_id, response_quality }): Json<ReviewQuery>) -> Json<ReviewResponse> {
    info!("Reviewing with {} quality", response_quality);
    knowledge.review_sentence(review_sentence_id, response_quality).await;

    Json(ReviewResponse {
        success: true
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Set RUST_LOG to info by default for other peoples' convenience.
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    // Create the knowledge database.
    let knowledge = knowledge::Knowledge::new().await;

    // A service serving static assets.
    let static_assets_serve = ServeDir::new("assets");

    // Create the routes.
    let app = Router::new()
        .route("/", get(index_get))
        .route("/review", get(review_get))
        .route("/review", post(review_post))
        .route("/add", get(add_get))
        .route("/add", post(add_post))
        .nest_service("/assets", static_assets_serve)
        .with_state(knowledge);

    // Start the server.
    axum::Server::bind(&"0.0.0.0:8000".parse().unwrap())
        .serve(app.into_make_service())
        .await?;

    Ok(())
}