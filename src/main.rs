use std::{net::SocketAddr, error::Error, sync::Arc, env, fmt::Display};
use serde::{Deserialize, Serialize};

use askama::Template;
use axum::{
    routing::{get, post},
    Router, extract::{State, Query}, Form, Json, response::IntoResponseParts,
};
use axum::http::{Uri, header, StatusCode};
use axum::response::{Response, IntoResponse};

use log::{info, error};
use tower_http::services::ServeDir;
use rust_embed::RustEmbed;

use clap::Parser;

mod knowledge;
use knowledge::Knowledge;

pub static STATIC_ASSETS_PATH: &str = concat!("/assets_", env!("CARGO_PKG_VERSION"));

// An error template
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    status: StatusCode,
    text: String
}

// Error type for our contorller
#[derive(Debug)]
pub enum ControllerError {
    KnowledgeError(knowledge::KnowledgeError),
    NotFound
}

impl From<knowledge::KnowledgeError> for ControllerError {
    fn from(value: knowledge::KnowledgeError) -> Self {
        Self::KnowledgeError(value)
    }
}

impl Display for ControllerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KnowledgeError(e) => write!(f, "Error accessing knowledge: {}", e),
            Self::NotFound => write!(f, "Not Found")
        }
    }
}

impl std::error::Error for ControllerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::KnowledgeError(e) => Some(e),
            Self::NotFound => None
        }
    }
}

impl IntoResponse for ControllerError {
    fn into_response(self) -> Response {
        match &self {
            Self::KnowledgeError(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR,
                ErrorTemplate {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    text: format!("{}", self).to_string()
                }).into_response()
            },
            Self::NotFound => {
                (StatusCode::NOT_FOUND,
                ErrorTemplate {
                    status: StatusCode::NOT_FOUND,
                    text: format!("{}", self).to_string()
                }).into_response()
            }
        }
    }
}

pub type ControllerResult<T> = Result<T, ControllerError>;

// Embed our assets
#[derive(RustEmbed)]
#[folder = "assets"]
struct Asset;

pub fn asset_routes() -> Router {
    Router::new().fallback(asset_handler)
}

async fn asset_handler(uri: Uri) -> ControllerResult<Response> {
    let path = uri.path()
        .trim_start_matches(STATIC_ASSETS_PATH)
        .trim_start_matches('/');

    if let Some(content) = Asset::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        Ok(([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response())
    }
    else {
        Err(ControllerError::NotFound)
    }
}

#[derive(Template)]
#[template(path = "add.html")]
struct AddTemplate {
}

async fn add_get(State(knowledge): State<Knowledge>) -> ControllerResult<AddTemplate> {
    Ok(AddTemplate { })
}

#[derive(Deserialize)]
struct AddTextQuery {
    text: String,
    source: String
}

#[derive(Serialize)]
struct AddTextResponse {
    success: bool,
    sentences_added: i64
}

async fn add_post(State(mut knowledge): State<Knowledge>,
                  Json(AddTextQuery{ text, source }): Json<AddTextQuery>) -> ControllerResult<Json<AddTextResponse>>
{
    let sentences_added = knowledge.add_text(text.as_str(), source.as_str()).await?;

    Ok(Json(AddTextResponse {
        success: true,
        sentences_added
    }))
}

#[derive(Template)]
#[template(path = "review.html")]
struct ReviewTemplate {
    sentence_id: i64,
    sentence: String,
    sentence_source: String,
    reviews_today_count: i64,
    words_being_reviewed: Vec<String>,
    words_that_are_new: Vec<String>
}

async fn review_get(State(knowledge): State<Knowledge>) -> ControllerResult<ReviewTemplate> {
    let review_info = knowledge.get_review_info().await?;
    let sentence_data = knowledge.get_next_sentence_i_plus_one().await?;

    Ok(ReviewTemplate {
        sentence_id: sentence_data.sentence_id,
        sentence: sentence_data.sentence_text,
        sentence_source: sentence_data.sentence_source,
        reviews_today_count: review_info.reviews_remaining,
        words_being_reviewed: sentence_data.words_being_reviewed.iter().map(|(_, text)| text.clone()).collect(),
        words_that_are_new: sentence_data.words_that_are_new.iter().map(|(_, text)| text.clone()).collect()
    })
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
                     Json(ReviewQuery{ review_sentence_id, response_quality }): Json<ReviewQuery>) -> ControllerResult<Json<ReviewResponse>> {
    info!("Reviewing with {} quality", response_quality);
    knowledge.review_sentence(review_sentence_id, response_quality).await?;

    Ok(Json(ReviewResponse {
        success: true
    }))
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    // Whether or not to re-tokenize sentences.
    #[arg(short, long)]
    retokenize: bool
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Set RUST_LOG to info by default for other peoples' convenience.
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    // Parse command line arguments
    let args = Args::parse();

    // Create the knowledge database.
    let mut knowledge = knowledge::Knowledge::new().await?;

    // Retokenize our db if specified.
    if args.retokenize {
        knowledge.retokenize().await?
    }

    // Create the routes.
    let app = Router::new()
        .route("/", get(review_get))
        .route("/review", post(review_post))
        .route("/add", get(add_get))
        .route("/add", post(add_post))
        .nest_service("/assets", asset_routes())
        .with_state(knowledge);

    // Start the server.
    axum::Server::bind(&"0.0.0.0:8000".parse().unwrap())
        .serve(app.into_make_service())
        .await?;

    Ok(())
}