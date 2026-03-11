use axum::{
    extract::State,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Parser, Debug, Clone)]
#[command(
    author,
    version,
    about = "GitAnalyser - A powerful tool to extract productivity metrics from your git repositories and visualize them via an interactive, neon-styled web dashboard.",
    long_about = "\
GitAnalyser reads your local git repository's commit history and file structures to calculate important productivity and code-base metrics.
It starts a local web server (defaulting to port 8080) and provides a dashboard where you can see:
  - Commit frequency over time.
  - Code churn (lines added vs deleted).
  - File hotspots (files that are modified most often).
  - Knowledge silos (files primarily owned by a single author).
  - Branch lifespans.

You can filter these metrics globally or by individual author directly in the web dashboard.

EXAMPLES:
  # Analyze the current directory and start the web server on the default port (8080):
  GitAnalyser

  # Analyze a specific repository folder and start the web server on port 3000:
  GitAnalyser --path /home/user/projects/my-repo --port 3000
"
)]
pub struct Args {
    /// Port to run the web server on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Path to the git repository folder
    #[arg(short, long, default_value = ".")]
    pub path: String,
}

mod metrics;

type AppState = Arc<metrics::RepositoryMetrics>;

#[tokio::main]
async fn main() {
    let args = Args::parse();
    println!("Analyzing repository at: {}", args.path);

    let metrics = match metrics::analyze_repository(&args.path) {
        Ok(metrics) => {
            println!("Successfully analyzed repository!");
            println!("Found {} commits.", metrics.commits.len());
            metrics
        }
        Err(e) => {
            eprintln!("Failed to analyze repository: {}", e);
            std::process::exit(1);
        }
    };

    let app_state = Arc::new(metrics);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/metrics", get(metrics_handler))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{}", args.port);
    println!("Starting server on http://{}", addr);

    let listener = TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn index_handler() -> impl IntoResponse {
    let html = include_str!("index.html");
    Html(html)
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json((*state).clone())
}
